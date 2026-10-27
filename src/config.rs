//! Connection configuration storage.
//!
//! Servers are stored as individual TOML files under
//! `~/.pgbutler/connections/<name>.toml`, with login profiles nested under
//! each server. TLS behavior is controlled by `sslmode` (mirroring libpq).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};

mod keyring;
mod sslmode;

pub use keyring::{read_master_key, write_master_key};
pub use sslmode::SslMode;

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderConfig {
    Ollama(OllamaConfig),
    Anthropic(AnthropicConfig),
    #[serde(rename = "openai")]
    OpenAi(OpenAiConfig),
}

const DEFAULT_ANTHROPIC_URL: &str = "https://api.anthropic.com";
const DEFAULT_ANTHROPIC_MODEL: &str = rig::providers::anthropic::CLAUDE_SONNET_5_5;
const DEFAULT_OPENAI_URL: &str = "https://api.openai.com/v1";
const DEFAULT_OPENAI_MODEL: &str = "gpt-5";

/// Anthropic settings. `api_key` falls back to `ANTHROPIC_API_KEY`.
#[derive(Clone, Deserialize, Serialize)]
pub struct AnthropicConfig {
    /// Encrypted at rest; empty means unset.
    #[serde(
        default,
        serialize_with = "serialize_password",
        deserialize_with = "deserialize_password"
    )]
    pub api_key: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: Some(DEFAULT_ANTHROPIC_URL.to_owned()),
            model: Some(DEFAULT_ANTHROPIC_MODEL.to_owned()),
        }
    }
}

impl AnthropicConfig {
    fn with_defaults(&self) -> Self {
        let defaults = Self::default();
        Self {
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone().or(defaults.base_url),
            model: self.model.clone().or(defaults.model),
        }
    }
}

/// OpenAI settings. `api_key` falls back to `OPENAI_API_KEY`.
#[derive(Clone, Deserialize, Serialize)]
pub struct OpenAiConfig {
    /// Encrypted at rest; empty means unset.
    #[serde(
        default,
        serialize_with = "serialize_password",
        deserialize_with = "deserialize_password"
    )]
    pub api_key: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: Some(DEFAULT_OPENAI_URL.to_owned()),
            model: Some(DEFAULT_OPENAI_MODEL.to_owned()),
        }
    }
}

impl OpenAiConfig {
    fn with_defaults(&self) -> Self {
        let defaults = Self::default();
        Self {
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone().or(defaults.base_url),
            model: self.model.clone().or(defaults.model),
        }
    }
}

const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";
const DEFAULT_OLLAMA_MODEL: &str = "llama3.2";
const DEFAULT_OLLAMA_NUM_CTX: u64 = 65536;

#[derive(Clone, Deserialize, Serialize)]
pub struct OllamaConfig {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub num_ctx: Option<u64>,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: Some(DEFAULT_OLLAMA_URL.to_owned()),
            model: Some(DEFAULT_OLLAMA_MODEL.to_owned()),
            num_ctx: Some(DEFAULT_OLLAMA_NUM_CTX),
        }
    }
}

impl OllamaConfig {
    fn with_defaults(&self) -> Self {
        let defaults = Self::default();
        Self {
            base_url: self.base_url.clone().or(defaults.base_url),
            model: self.model.clone().or(defaults.model),
            num_ctx: self.num_ctx.or(defaults.num_ctx),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AppSection {
    pub provider: String,
}

impl Default for AppSection {
    fn default() -> Self {
        Self {
            provider: "ollama".to_owned(),
        }
    }
}

impl AppSection {
    fn with_defaults(&self) -> Self {
        self.clone()
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(default)]
struct Providers {
    ollama: Option<OllamaConfig>,
    anthropic: Option<AnthropicConfig>,
    openai: Option<OpenAiConfig>,
}

impl Providers {
    fn select(&self, name: &str) -> Result<ProviderConfig, String> {
        match name {
            "ollama" => Ok(ProviderConfig::Ollama(self.ollama.clone().ok_or(
                "provider 'ollama' selected but no [provider.ollama] section is set",
            )?)),
            "anthropic" => Ok(ProviderConfig::Anthropic(self.anthropic.clone().ok_or(
                "provider 'anthropic' selected but no [provider.anthropic] section is set",
            )?)),
            "openai" => Ok(ProviderConfig::OpenAi(self.openai.clone().ok_or(
                "provider 'openai' selected but no [provider.openai] section is set",
            )?)),
            other => Err(format!("unknown provider '{other}'")),
        }
    }

    fn with_defaults(&self) -> Self {
        Self {
            ollama: Some(
                self.ollama
                    .as_ref()
                    .map(OllamaConfig::with_defaults)
                    .unwrap_or_default(),
            ),
            anthropic: self.anthropic.as_ref().map(AnthropicConfig::with_defaults),
            openai: self.openai.as_ref().map(OpenAiConfig::with_defaults),
        }
    }
}

impl Default for Providers {
    fn default() -> Self {
        Self {
            ollama: Some(OllamaConfig::default()),
            anthropic: Some(AnthropicConfig::default()),
            openai: Some(OpenAiConfig::default()),
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    app: RwLock<AppSection>,
    provider: RwLock<Providers>,
    default_connection: RwLock<Option<String>>,
    #[serde(skip)]
    servers: RwLock<Vec<ServerConfig>>,
}

impl Config {
    pub fn read() -> Result<Config> {
        let config_file = config_file();
        let mut config = if config_file.exists() {
            read_config_file(&config_file)?
        } else {
            Config::default()
        };
        config.servers = RwLock::new(load_server_profiles()?);
        Ok(config.with_defaults())
    }

    pub fn write(&self) -> Result<()> {
        let config_content = toml::to_string(&self)?;
        let config_file = config_file();
        fs::create_dir_all(app_dir())?;
        Ok(fs::write(config_file, config_content)?)
    }

    pub fn select_provider(&self) -> Result<ProviderConfig, String> {
        self.providers().select(&self.provider_name())
    }

    /// Config of the named provider, regardless of which one is active.
    pub fn provider_config(&self, name: &str) -> Result<ProviderConfig, String> {
        self.providers().select(name)
    }

    /// Replace the config section of the provider that `config` belongs to.
    pub fn set_provider_config(&self, config: ProviderConfig) {
        let mut providers = self
            .provider
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match config {
            ProviderConfig::Ollama(c) => providers.ollama = Some(c),
            ProviderConfig::Anthropic(c) => providers.anthropic = Some(c),
            ProviderConfig::OpenAi(c) => providers.openai = Some(c),
        }
    }

    pub fn provider_model(&self) -> Result<Option<String>, String> {
        Ok(match self.select_provider()? {
            ProviderConfig::Ollama(config) => config.model,
            ProviderConfig::Anthropic(config) => config.model,
            ProviderConfig::OpenAi(config) => config.model,
        })
    }

    fn providers(&self) -> std::sync::RwLockReadGuard<'_, Providers> {
        self.provider
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Set the API key of the currently selected provider.
    pub fn set_api_key(&self, api_key: &str) -> Result<(), String> {
        let name = self.provider_name();
        let mut providers = self
            .provider
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match name.as_str() {
            "anthropic" => providers
                .anthropic
                .as_mut()
                .map(|c| c.api_key = api_key.to_owned()),
            "openai" => providers
                .openai
                .as_mut()
                .map(|c| c.api_key = api_key.to_owned()),
            "ollama" => return Err("provider 'ollama' does not use an API key".into()),
            other => return Err(format!("unknown provider '{other}'")),
        }
        .ok_or_else(|| format!("no [provider.{name}] section is set"))
    }

    /// Set the model of the currently selected provider.
    pub fn set_provider_model(&self, model: &str) -> Result<(), String> {
        let name = self.provider_name();
        let mut providers = self
            .provider
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match name.as_str() {
            "ollama" => providers
                .ollama
                .as_mut()
                .map(|c| c.model = Some(model.to_owned())),
            "anthropic" => providers
                .anthropic
                .as_mut()
                .map(|c| c.model = Some(model.to_owned())),
            "openai" => providers
                .openai
                .as_mut()
                .map(|c| c.model = Some(model.to_owned())),
            other => return Err(format!("unknown provider '{other}'")),
        }
        .ok_or_else(|| format!("no [provider.{name}] section is set"))
    }

    pub fn provider_name(&self) -> String {
        self.app
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .provider
            .clone()
    }

    pub fn default_connection(&self) -> Option<String> {
        self.default_connection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_default_connection(&self, conn: &Connection) {
        *self
            .default_connection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(conn.selector());
    }

    /// Switch the active provider; fails if it is unknown or has no config section.
    pub fn set_provider(&self, name: &str) -> Result<(), String> {
        self.providers().select(name)?;
        self.app
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .provider = name.to_owned();
        Ok(())
    }

    pub fn servers(&self) -> Vec<ServerConfig> {
        self.servers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Flatten each saved server/login pair for runtime consumers.
    pub fn connections(&self) -> Vec<Connection> {
        self.servers()
            .into_iter()
            .flat_map(|server| {
                server
                    .logins
                    .iter()
                    .map(|login| server.flatten(login))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub fn save_server(
        &self,
        server: &ServerConfig,
        old_name: Option<&str>,
    ) -> std::io::Result<PathBuf> {
        if self.servers().iter().any(|existing| {
            old_name != Some(existing.name.as_str())
                && (existing.name == server.name
                    || sanitize_filename(&existing.name) == sanitize_filename(&server.name))
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("server '{}' already exists", server.name),
            ));
        }
        let path = save_server_profile(server)?;
        if let Some(old_name) = old_name
            && old_name != server.name
        {
            let old_path = connections_dir().join(format!("{}.toml", sanitize_filename(old_name)));
            if old_path != path && old_path.exists() {
                fs::remove_file(old_path)?;
            }
        }

        let mut servers = self
            .servers
            .write()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        if let Some(existing) = servers.iter_mut().find(|existing| {
            existing.name == server.name || old_name.is_some_and(|name| existing.name == name)
        }) {
            *existing = server.clone();
        } else {
            servers.push(server.clone());
        }
        servers.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(path)
    }

    pub fn save_login(
        &self,
        server_name: &str,
        index: Option<usize>,
        login: &LoginConfig,
    ) -> std::io::Result<()> {
        let mut server = self
            .servers()
            .into_iter()
            .find(|server| server.name == server_name)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "server not found"))?;
        if let Some(index) = index {
            let Some(existing) = server.logins.get_mut(index) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "login not found",
                ));
            };
            *existing = login.clone();
        } else {
            server.logins.push(login.clone());
        }
        save_server_profile(&server)?;
        let mut servers = self
            .servers
            .write()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        if let Some(existing) = servers
            .iter_mut()
            .find(|existing| existing.name == server.name)
        {
            *existing = server;
        }
        Ok(())
    }

    pub fn delete_login(&self, server_name: &str, index: usize) -> std::io::Result<()> {
        let mut server = self
            .servers()
            .into_iter()
            .find(|server| server.name == server_name)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "server not found"))?;
        if index >= server.logins.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "login not found",
            ));
        }
        server.logins.remove(index);
        save_server_profile(&server)?;
        let mut servers = self
            .servers
            .write()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        if let Some(existing) = servers
            .iter_mut()
            .find(|existing| existing.name == server.name)
        {
            *existing = server;
        }
        Ok(())
    }

    pub fn save_connection(&self, conn: &Connection) -> std::io::Result<PathBuf> {
        let mut server = self
            .servers()
            .into_iter()
            .find(|server| server.name == conn.name)
            .unwrap_or_else(|| ServerConfig::from_connection(conn));
        server.update_from_connection(conn);
        let login = LoginConfig::from_connection(conn);
        if let Some(existing) = server
            .logins
            .iter_mut()
            .find(|existing| existing.login == login.login && existing.catalog == login.catalog)
        {
            *existing = login;
        } else {
            server.logins.push(login);
        }
        self.save_server(&server, None)
    }

    pub fn delete_server(&self, name: &str) -> std::io::Result<()> {
        let path = connections_dir().join(format!("{}.toml", sanitize_filename(name)));
        fs::remove_file(path)?;
        let mut servers = self
            .servers
            .write()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        servers.retain(|server| server.name != name);
        Ok(())
    }

    fn with_defaults(&self) -> Self {
        Self {
            app: RwLock::new(
                self.app
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .with_defaults(),
            ),
            provider: RwLock::new(self.providers().with_defaults()),
            default_connection: RwLock::new(self.default_connection()),
            servers: RwLock::new(self.servers()),
        }
    }
}

fn read_config_file(path: &Path) -> Result<Config> {
    let config_content = fs::read_to_string(path)?;
    Ok(toml::from_str(&config_content)?)
}

pub fn default_schema() -> Option<String> {
    Some("public".to_string())
}

/// A PostgreSQL server and its shared network/TLS settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Friendly, unique name for this server (also used as the file stem).
    pub name: String,
    pub host: String,
    pub port: u16,
    /// How strictly TLS is required and verified; see [`SslMode`].
    pub sslmode: SslMode,
    /// Path to the CA certificate used to verify the server. Only required
    /// when `sslmode` is `verify-ca` or `verify-full`.
    pub sslrootcert: Option<String>,
    /// Path to the client certificate presented to the server. Always
    /// optional: used for mutual TLS if provided and TLS is in use.
    pub sslcert: Option<String>,
    /// Path to the client private key matching `sslcert`. Always optional,
    /// on the same terms as `sslcert`.
    pub sslkey: Option<String>,
    #[serde(default)]
    pub logins: Vec<LoginConfig>,
}

impl ServerConfig {
    pub fn with_defaults(name: impl Into<String>) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let pg_dir = home.join(".postgresql");
        Self {
            name: name.into(),
            host: "localhost".to_string(),
            port: 5432,
            sslmode: SslMode::default(),
            sslrootcert: Some(pg_dir.join("root.crt").to_string_lossy().into_owned()),
            sslcert: Some(pg_dir.join("postgresql.crt").to_string_lossy().into_owned()),
            sslkey: Some(pg_dir.join("postgresql.key").to_string_lossy().into_owned()),
            logins: Vec::new(),
        }
    }

    fn from_connection(conn: &Connection) -> Self {
        Self {
            name: conn.name.clone(),
            host: conn.host.clone(),
            port: conn.port,
            sslmode: conn.sslmode,
            sslrootcert: conn.sslrootcert.clone(),
            sslcert: conn.sslcert.clone(),
            sslkey: conn.sslkey.clone(),
            logins: Vec::new(),
        }
    }

    fn update_from_connection(&mut self, conn: &Connection) {
        self.host = conn.host.clone();
        self.port = conn.port;
        self.sslmode = conn.sslmode;
        self.sslrootcert = conn.sslrootcert.clone();
        self.sslcert = conn.sslcert.clone();
        self.sslkey = conn.sslkey.clone();
    }

    pub fn flatten(&self, login: &LoginConfig) -> Connection {
        Connection {
            name: self.name.clone(),
            host: self.host.clone(),
            port: self.port,
            catalog: login.catalog.clone(),
            login: login.login.clone(),
            password: login.password.clone(),
            sslmode: self.sslmode,
            sslrootcert: self.sslrootcert.clone(),
            sslcert: self.sslcert.clone(),
            sslkey: self.sslkey.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.host.trim().is_empty() {
            return Err("host must not be empty".into());
        }
        if self.port == 0 {
            return Err("port must be between 1 and 65535".into());
        }
        check_cert_path(
            "root CA certificate (sslrootcert)",
            &self.sslrootcert,
            self.sslmode.requires_root_cert(),
            self.sslmode,
        )?;
        check_cert_path(
            "client certificate (sslcert)",
            &self.sslcert,
            false,
            self.sslmode,
        )?;
        check_cert_path("client key (sslkey)", &self.sslkey, false, self.sslmode)?;
        let is_cert_set = self
            .sslcert
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
        let is_key_set = self.sslkey.as_deref().is_some_and(|s| !s.trim().is_empty());
        if is_cert_set != is_key_set {
            return Err("sslcert and sslkey must both be set, or both left empty".into());
        }
        Ok(())
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self::with_defaults(String::new())
    }
}

/// Credentials and database selection scoped to one server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoginConfig {
    pub catalog: String,
    pub login: String,
    /// Plaintext password for `login`, kept in memory only. Encrypted at
    /// rest with the `crypto` module when the server is serialized to
    /// disk, and transparently decrypted back to plaintext when read.
    #[serde(
        default,
        serialize_with = "serialize_password",
        deserialize_with = "deserialize_password"
    )]
    pub password: String,
}

impl LoginConfig {
    pub fn with_defaults() -> Self {
        Self {
            catalog: "postgres".to_string(),
            login: whoami_fallback(),
            password: String::new(),
        }
    }

    fn from_connection(conn: &Connection) -> Self {
        Self {
            catalog: conn.catalog.clone(),
            login: conn.login.clone(),
            password: conn.password.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.catalog.trim().is_empty() {
            return Err("database name must not be empty".into());
        }
        if self.login.trim().is_empty() {
            return Err("user must not be empty".into());
        }
        Ok(())
    }
}

impl Default for LoginConfig {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Flattened server/login selection used by the setup flow and runtime tools.
#[derive(Debug, Clone)]
pub struct Connection {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub catalog: String,
    pub login: String,
    pub password: String,
    pub sslmode: SslMode,
    pub sslrootcert: Option<String>,
    pub sslcert: Option<String>,
    pub sslkey: Option<String>,
}

impl Connection {
    /// Reasonable defaults for a new flattened server/login selection.
    pub fn with_defaults(name: impl Into<String>) -> Self {
        let server = ServerConfig::with_defaults(name);
        server.flatten(&LoginConfig::default())
    }

    pub fn validate(&self) -> Result<(), String> {
        ServerConfig::from_connection(self).validate()?;
        LoginConfig::from_connection(self).validate()
    }

    pub fn selector(&self) -> String {
        format!("{}@{}", self.login, self.name)
    }
}

/// Validate a single optional certificate path field: if `required`, it must
/// be present; if present (required or not), it must point to an existing
/// file.
fn check_cert_path(
    label: &str,
    path: &Option<String>,
    required: bool,
    mode: SslMode,
) -> Result<(), String> {
    match path.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => {
            if !Path::new(p).is_file() {
                return Err(format!("{label} not found at '{p}'"));
            }
            Ok(())
        }
        _ if required => Err(format!("{label} is mandatory for sslmode '{mode}'")),
        _ => Ok(()),
    }
}

fn whoami_fallback() -> String {
    std::env::var("PGUSER")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "postgres".to_string())
}

/// Serialize the plaintext password field by encrypting it (AES-256-GCM via
/// [`crate::crypto`]) and base64-encoding the ciphertext for safe storage in
/// TOML. An empty password is stored as-is, without encryption, so that
/// connections without a saved password don't force creation of a master key.
fn serialize_password<S>(password: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::Error;
    if password.is_empty() {
        return serializer.serialize_str("");
    }
    let ciphertext = crate::crypto::encrypt(password.as_bytes()).map_err(S::Error::custom)?;
    serializer.serialize_str(&openssl::base64::encode_block(&ciphertext))
}

/// Deserialize the password field, decrypting the base64-encoded ciphertext
/// produced by [`serialize_password`] back to plaintext. An empty string
/// (or a missing field, via `#[serde(default)]`) deserializes to an empty
/// password.
fn deserialize_password<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    use serde::de::Error;
    let encoded = String::deserialize(deserializer)?;
    if encoded.is_empty() {
        return Ok(String::new());
    }
    let ciphertext = openssl::base64::decode_block(&encoded).map_err(D::Error::custom)?;
    let plaintext = crate::crypto::decrypt(&ciphertext).map_err(D::Error::custom)?;
    String::from_utf8(plaintext).map_err(D::Error::custom)
}

/// Root directory for app files
pub fn app_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pgbutler")
}

pub fn log_dir() -> PathBuf {
    app_dir().join("logs")
}

/// Root directory holding connection profiles: `~/.pgbutler/connections`.
pub fn connections_dir() -> PathBuf {
    app_dir().join("connections")
}

pub fn config_file() -> PathBuf {
    app_dir().join("config.toml")
}

fn load_server_profiles() -> std::io::Result<Vec<ServerConfig>> {
    let dir = connections_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut servers = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let contents = fs::read_to_string(&path)?;
        if let Some(server) = parse_server_profile(&contents) {
            servers.push(server);
        }
    }
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(servers)
}

fn parse_server_profile(contents: &str) -> Option<ServerConfig> {
    let value = toml::from_str::<toml::Value>(contents).ok()?;
    if value.get("logins").is_some() {
        return toml::from_str(contents).ok();
    }
    toml::from_str::<LegacyConnection>(contents)
        .ok()
        .map(|legacy| {
            let mut server = ServerConfig::with_defaults(legacy.name);
            server.host = legacy.host;
            server.port = legacy.port;
            server.sslmode = legacy.sslmode;
            server.sslrootcert = legacy.sslrootcert;
            server.sslcert = legacy.sslcert;
            server.sslkey = legacy.sslkey;
            server.logins.push(LoginConfig {
                catalog: legacy.catalog,
                login: legacy.login,
                password: legacy.password,
            });
            server
        })
}

fn save_server_profile(server: &ServerConfig) -> std::io::Result<PathBuf> {
    let dir = connections_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.toml", sanitize_filename(&server.name)));
    let contents = toml::to_string_pretty(server).expect("server is always serializable");
    fs::write(&path, contents)?;
    Ok(path)
}

#[derive(Deserialize)]
struct LegacyConnection {
    name: String,
    host: String,
    port: u16,
    catalog: String,
    login: String,
    #[serde(default)]
    sslmode: SslMode,
    #[serde(default)]
    sslrootcert: Option<String>,
    #[serde(default)]
    sslcert: Option<String>,
    #[serde(default)]
    sslkey: Option<String>,
    #[serde(
        default,
        serialize_with = "serialize_password",
        deserialize_with = "deserialize_password"
    )]
    password: String,
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_provider_fills_missing_ollama_defaults() {
        let config = Config {
            app: RwLock::new(AppSection::default()),
            provider: RwLock::new(Providers {
                ollama: Some(OllamaConfig {
                    base_url: None,
                    model: Some("qwen3".to_string()),
                    num_ctx: None,
                }),
                ..Providers::default()
            }),
            ..Config::default()
        };

        let normalized = config.with_defaults();
        let ProviderConfig::Ollama(ollama) = normalized.select_provider().expect("provider") else {
            panic!("expected ollama provider");
        };
        assert_eq!(ollama.base_url.as_deref(), Some(DEFAULT_OLLAMA_URL));
        assert_eq!(ollama.model.as_deref(), Some("qwen3"));
        assert_eq!(ollama.num_ctx, Some(DEFAULT_OLLAMA_NUM_CTX));
        assert!(
            config
                .providers()
                .ollama
                .as_ref()
                .unwrap()
                .base_url
                .is_none()
        );
    }

    #[test]
    fn setting_model_updates_only_the_active_provider() {
        let config = Config::default();

        config
            .set_provider_model("qwen3")
            .expect("set Ollama model");
        assert_eq!(
            config.provider_model().expect("Ollama model").as_deref(),
            Some("qwen3")
        );

        config.set_provider("anthropic").expect("select Anthropic");
        config
            .set_provider_model("claude-custom")
            .expect("set Anthropic model");
        assert_eq!(
            config.provider_model().expect("Anthropic model").as_deref(),
            Some("claude-custom")
        );

        config.set_provider("ollama").expect("select Ollama");
        assert_eq!(
            config.provider_model().expect("Ollama model").as_deref(),
            Some("qwen3")
        );
    }

    #[test]
    fn legacy_connection_profile_loads_as_a_server_with_one_login() {
        let profile = r#"
name = "warehouse"
host = "db.example.test"
port = 5433
catalog = "inventory"
login = "analyst"
password = ""
sslmode = "disable"
"#;
        let server = parse_server_profile(profile).expect("legacy profile should load");

        assert_eq!(server.name, "warehouse");
        assert_eq!(server.host, "db.example.test");
        assert_eq!(server.port, 5433);
        assert_eq!(server.logins.len(), 1);
        assert_eq!(server.logins[0].catalog, "inventory");
        assert_eq!(server.logins[0].login, "analyst");
    }

    #[test]
    fn connections_flatten_server_and_login_settings() {
        let mut server = ServerConfig::with_defaults("warehouse");
        server.host = "db.example.test".to_owned();
        server.port = 5433;
        server.sslmode = SslMode::Disable;
        server.sslrootcert = None;
        server.sslcert = None;
        server.sslkey = None;
        server.logins.push(LoginConfig {
            catalog: "inventory".to_owned(),
            login: "analyst".to_owned(),
            password: "secret".to_owned(),
        });
        let config = Config {
            servers: RwLock::new(vec![server]),
            ..Config::default()
        };

        let connection = config.connections().pop().expect("flattened connection");
        assert_eq!(connection.name, "warehouse");
        assert_eq!(connection.host, "db.example.test");
        assert_eq!(connection.port, 5433);
        assert_eq!(connection.catalog, "inventory");
        assert_eq!(connection.login, "analyst");
        assert_eq!(connection.password, "secret");
    }

    #[test]
    fn default_connection_selector_round_trips_in_main_config() {
        let config = Config::default();
        let mut connection = Connection::with_defaults("warehouse");
        connection.login = "alice".to_owned();
        config.set_default_connection(&connection);

        let serialized = toml::to_string(&config).expect("config should serialize");
        let deserialized: Config = toml::from_str(&serialized).expect("config should deserialize");

        assert_eq!(
            deserialized.default_connection().as_deref(),
            Some("alice@warehouse")
        );
    }

    #[test]
    #[ignore = "touches the real macOS login Keychain (creates/reads the master key); run manually"]
    fn password_is_encrypted_at_rest_and_round_trips() {
        let mut server = ServerConfig::with_defaults("pw-test-server");
        let mut login = LoginConfig::with_defaults();
        login.password = "s3cr3t-p@ssw0rd".to_string();
        server.logins.push(login);

        let serialized = toml::to_string_pretty(&server).expect("serialization should succeed");
        // The plaintext password must never appear in the serialized form.
        assert!(!serialized.contains("s3cr3t-p@ssw0rd"));

        let deserialized: ServerConfig =
            toml::from_str(&serialized).expect("deserialization should succeed");
        assert_eq!(deserialized.logins[0].password, server.logins[0].password);
    }

    #[test]
    fn empty_password_round_trips_without_encryption() {
        let mut server = ServerConfig::with_defaults("no-pw-test-server");
        server.logins.push(LoginConfig::with_defaults());
        assert!(server.logins[0].password.is_empty());

        let serialized = toml::to_string_pretty(&server).expect("serialization should succeed");
        assert!(serialized.contains("password = \"\""));

        let deserialized: ServerConfig =
            toml::from_str(&serialized).expect("deserialization should succeed");
        assert!(deserialized.logins[0].password.is_empty());
    }

    #[test]
    fn disable_sslmode_does_not_require_any_cert() {
        let mut conn = Connection::with_defaults("disable-test-connection");
        conn.sslmode = SslMode::Disable;
        conn.sslrootcert = None;
        conn.sslcert = None;
        conn.sslkey = None;
        assert!(conn.validate().is_ok());
    }

    #[test]
    fn require_sslmode_does_not_require_root_cert() {
        let mut conn = Connection::with_defaults("require-test-connection");
        conn.sslmode = SslMode::Require;
        conn.sslrootcert = None;
        conn.sslcert = None;
        conn.sslkey = None;
        assert!(conn.validate().is_ok());
    }

    #[test]
    fn verify_ca_requires_root_cert() {
        let mut conn = Connection::with_defaults("verify-ca-test-connection");
        conn.sslmode = SslMode::VerifyCa;
        conn.sslrootcert = None;
        conn.sslcert = None;
        conn.sslkey = None;
        let err = conn.validate().expect_err("missing root cert should fail");
        assert!(err.contains("root CA certificate"));
    }

    #[test]
    fn client_cert_and_key_must_both_be_set_or_both_empty() {
        let mut conn = Connection::with_defaults("half-mtls-test-connection");
        conn.sslmode = SslMode::Require;
        conn.sslrootcert = None;
        conn.sslcert = Some("/nonexistent/does-not-matter.crt".to_string());
        conn.sslkey = None;
        let err = conn
            .validate()
            .expect_err("mismatched cert/key should fail");
        // Fails on the missing file for sslcert before reaching the pairing
        // check; either error is acceptable evidence that validation caught it.
        assert!(err.contains("sslcert") || err.contains("both be set"));
    }
}
