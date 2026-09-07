//! Master encryption key storage in the platform's native secure credential
//! store (macOS Keychain), falling back to a restricted-permission file on
//! other platforms.

/// Service/account identifiers under which the master encryption key is
/// stored in the platform credential store (macOS Keychain).
const MASTER_KEY_SERVICE: &str = "pgbutler";
const MASTER_KEY_ACCOUNT: &str = "master-encryption-key";

/// Read the master encryption key from the OS-native secure store.
///
/// On macOS this reads a generic password item from the login Keychain
/// (service `pgbutler`, account `master-encryption-key`). If the item's
/// access control list doesn't already trust this binary, macOS shows its
/// standard "pgbutler wants to use your confidential information stored in
/// keychain ... Allow / Always Allow / Deny" dialog, requiring the user's
/// login password or Touch ID before the key is released.
///
/// Returns `Ok(None)` if no master key has been stored yet.
pub fn read_master_key() -> Result<Option<String>, String> {
    master_key_store::read()
}

/// Write (create or update) the master encryption key in the OS-native
/// secure store. See [`read_master_key`] for details on the storage
/// location and the access-control prompts this can trigger on macOS.
pub fn write_master_key(key: &str) -> Result<(), String> {
    master_key_store::write(key)
}

#[cfg(target_os = "macos")]
mod master_key_store {
    use std::sync::OnceLock;

    use apple_native_keyring_store::keychain;
    use keyring_core::{Entry, Error as KeyringError};

    use super::{MASTER_KEY_ACCOUNT, MASTER_KEY_SERVICE};

    /// Lazily register the macOS login-Keychain credential store as the
    /// default `keyring-core` store. This is idempotent and safe to call
    /// on every read/write.
    fn ensure_store() -> Result<(), String> {
        static INIT: OnceLock<Result<(), String>> = OnceLock::new();
        INIT.get_or_init(|| {
            let store = keychain::Store::new()
                .map_err(|e| format!("failed to open macOS Keychain: {e}"))?;
            keyring_core::set_default_store(store);
            Ok(())
        })
        .clone()
    }

    fn entry() -> Result<Entry, String> {
        ensure_store()?;
        Entry::new(MASTER_KEY_SERVICE, MASTER_KEY_ACCOUNT)
            .map_err(|e| format!("failed to open Keychain entry: {e}"))
    }

    /// Read the master key, prompting the user for Keychain access (via the
    /// standard macOS password/Touch ID dialog) if required by the item's
    /// access control list.
    pub fn read() -> Result<Option<String>, String> {
        match entry()?.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(format!("failed to read master key from Keychain: {e}")),
        }
    }

    /// Write the master key, prompting the user for Keychain access (via the
    /// standard macOS password/Touch ID dialog) if required.
    pub fn write(key: &str) -> Result<(), String> {
        entry()?
            .set_password(key)
            .map_err(|e| format!("failed to write master key to Keychain: {e}"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        #[ignore = "touches the real macOS login Keychain and may show a permission prompt; run manually"]
        fn roundtrip_master_key() {
            let key = "pgbutler-test-master-key-12345";
            write(key).expect("write should succeed");
            let readback = read().expect("read should succeed");
            assert_eq!(readback.as_deref(), Some(key));

            // Clean up so repeated runs start from a known state.
            entry()
                .unwrap()
                .delete_credential()
                .expect("cleanup should succeed");
            let after_delete = read().expect("read after delete should succeed");
            assert_eq!(after_delete, None);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod master_key_store {
    //! Fallback store for non-macOS platforms: a single file under
    //! `~/.pgbutler`, created with owner-only (0600) permissions. This does
    //! not use an OS-native secure store; native credential-manager
    //! integration (e.g. Windows Credential Manager, Secret Service) can be
    //! added here later.

    use std::fs;
    use std::path::PathBuf;

    fn key_file_path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".pgbutler").join("master.key")
    }

    pub fn read() -> Result<Option<String>, String> {
        let path = key_file_path();
        if !path.is_file() {
            return Ok(None);
        }
        fs::read_to_string(&path)
            .map(|s| Some(s.trim_end().to_string()))
            .map_err(|e| format!("failed to read master key file '{}': {e}", path.display()))
    }

    pub fn write(key: &str) -> Result<(), String> {
        let path = key_file_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create '{}': {e}", parent.display()))?;
        }
        fs::write(&path, key)
            .map_err(|e| format!("failed to write master key file '{}': {e}", path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&path, perms).map_err(|e| {
                format!(
                    "failed to restrict permissions on '{}': {e}",
                    path.display()
                )
            })?;
        }
        Ok(())
    }
}
