mod agent;
mod app;
mod config;
mod crypto;
mod db;
mod theme;
mod ui;

use app::App;
use chrono::Local;
use logforth::{
    append::file::FileBuilder,
    layout::TextLayout,
    record::{Level, LevelFilter},
};
use std::io::stdout;

#[cfg(unix)]
struct KeyboardEnhancementGuard {
    enabled: bool,
}

#[cfg(unix)]
impl KeyboardEnhancementGuard {
    fn enable_if_supported() -> Self {
        use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
        use crossterm::terminal::supports_keyboard_enhancement;

        let enabled = match supports_keyboard_enhancement() {
            Ok(true) => match crossterm::execute!(
                stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            ) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("Could not enable enhanced keyboard reporting: {error}");
                    false
                }
            },
            Ok(false) => {
                log::info!("Terminal does not support enhanced keyboard reporting");
                false
            }
            Err(error) => {
                log::warn!("Could not detect enhanced keyboard reporting support: {error}");
                false
            }
        };

        Self { enabled }
    }
}

#[cfg(unix)]
impl Drop for KeyboardEnhancementGuard {
    fn drop(&mut self) {
        if self.enabled {
            use crossterm::event::PopKeyboardEnhancementFlags;

            if let Err(error) = crossterm::execute!(stdout(), PopKeyboardEnhancementFlags) {
                log::warn!("Could not restore terminal keyboard reporting: {error}");
            }
        }
    }
}

#[cfg(not(unix))]
struct KeyboardEnhancementGuard;

#[cfg(not(unix))]
impl KeyboardEnhancementGuard {
    fn enable_if_supported() -> Self {
        Self
    }
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let now = Local::now();
    let log_name = format!("{}", now.format("%Y-%m-%dT%H-%M-%S"));
    let log_file = FileBuilder::new(crate::config::log_dir(), log_name)
        .filename_suffix("log")
        .layout(TextLayout::default())
        .build()
        .unwrap();

    logforth::starter_log::builder()
        .dispatch(|d| {
            d.filter(LevelFilter::MoreSevereEqual(Level::Debug))
                .append(log_file)
        })
        .apply();

    log::info!("Starting pgbutler...");

    color_eyre::install()?;

    let terminal = ratatui::init();
    let keyboard_enhancement = KeyboardEnhancementGuard::enable_if_supported();
    let result = App::new().run(terminal).await;
    drop(keyboard_enhancement);
    ratatui::restore();

    log::info!("Existing pgbutler");

    result
}
