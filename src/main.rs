mod agent;
mod app;
mod config;
mod crypto;
mod db;
mod theme;
mod ui;

use app::App;
use chrono::Local;
use logforth::{append::file::FileBuilder, layout::TextLayout, record::{Level, LevelFilter}};

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
        .dispatch(|d| d.filter(LevelFilter::MoreSevereEqual(Level::Debug)).append(log_file))
        .apply();

    log::info!("Starting pgbutler...");

    color_eyre::install()?;

    let terminal = ratatui::init();
    let result = App::new().run(terminal).await;
    ratatui::restore();

    log::info!("Existing pgbutler");

    result
}
