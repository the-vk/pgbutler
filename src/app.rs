//! Application state machine and the terminal event loop.

use std::sync::Arc;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::{FutureExt, StreamExt};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;
use tokio_postgres::Client;

use crate::agent;
use crate::config::{self, Connection};
use crate::db::{self, QueryOutcome, RelKind};
use crate::store::Store;
use crate::ui::chat::{ChatAction, ChatScreen};
use crate::ui::setup::SetupScreen;

/// Top-level screen currently being shown.
pub enum Screen {
    Setup(SetupScreen),
    Chat(ChatScreen),
}

/// Events produced by background async work and fed back into the main loop.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Terminal error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Config(#[from] crate::config::ConfigError),
}

pub enum AppEvent {
    ConnectResult {
        conn: Box<Connection>,
        result: Result<(Client, Store), String>,
    },
    QueryResult(Result<QueryOutcome, String>),
    ExplainResult(Result<String, String>),
    AnalyzeProgress(agent::AnalysisProgress),
    AnalyzeResult(Result<agent::Response, String>),
    RelKindResult(Result<RelKind, String>),
    ModelsResult(Result<Vec<String>, String>),
    ProviderModelsResult(String, Result<Vec<String>, String>),
    ProviderReady(bool),
}

pub struct App {
    pub screen: Screen,
    pub should_quit: bool,
    config: Arc<config::Config>,
    events_tx: mpsc::UnboundedSender<AppEvent>,
    events_rx: mpsc::UnboundedReceiver<AppEvent>,
}

impl App {
    /// Build the initial app state, preferring the saved default connection.
    pub fn new(config: Arc<config::Config>) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let connections = config.connections();
        let default_connection = config.default_connection();

        let selected = default_connection
            .as_ref()
            .and_then(|selector| {
                connections
                    .iter()
                    .find(|connection| connection.selector() == *selector)
                    .cloned()
            })
            .or_else(|| connections.into_iter().next());

        let screen = if let Some(conn) = selected {
            let mut setup = SetupScreen::from_connection(conn);
            setup.autoconnect = true;
            Screen::Setup(setup)
        } else {
            Screen::Setup(SetupScreen::new())
        };

        Self {
            screen,
            should_quit: false,
            config,
            events_tx,
            events_rx,
        }
    }

    pub async fn run(&mut self, mut terminal: DefaultTerminal) -> Result<(), AppError> {
        let mut reader = EventStream::new();

        // Kick off an immediate connect attempt if a saved connection should
        // be tried automatically on startup.
        let autoconnect_target = if let Screen::Setup(setup) = &mut self.screen {
            if setup.autoconnect {
                setup.autoconnect = false;
                setup.connecting = true;
                Some(setup.draft())
            } else {
                None
            }
        } else {
            None
        };
        if let Some(conn) = autoconnect_target {
            self.spawn_connect(conn);
        }

        while !self.should_quit {
            terminal.draw(|frame| match &mut self.screen {
                Screen::Setup(s) => s.draw(frame),
                Screen::Chat(s) => s.draw(frame),
            })?;

            tokio::select! {
                maybe_event = reader.next().fuse() => {
                    if let Some(Ok(event)) = maybe_event {
                        self.handle_terminal_event(event);
                    }
                }
                Some(app_event) = self.events_rx.recv() => {
                    self.handle_app_event(app_event);
                }
            }
        }
        self.config.write()?;
        Ok(())
    }

    fn handle_terminal_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return;
                }
                if is_quit(&key) {
                    self.should_quit = true;
                    return;
                }
                match &mut self.screen {
                    Screen::Setup(setup) => {
                        if let Some(draft) = setup.handle_key(key) {
                            setup.connecting = true;
                            self.spawn_connect(draft);
                        }
                    }
                    Screen::Chat(chat) => {
                        if let Some(action) = chat.handle_key(key) {
                            match action {
                                ChatAction::Query(sql) => self.spawn_query(sql),
                                ChatAction::Quit => self.should_quit = true,
                                ChatAction::Connect(conn) => self.spawn_connect(conn),
                                ChatAction::Explain(sql) => self.spawn_explain(sql),
                                ChatAction::Analyze(sql) => self.spawn_analyze(sql),
                                ChatAction::RelKind(schema, relation) => {
                                    self.spawn_relkind(schema, relation)
                                }
                                ChatAction::RefreshModels => self.spawn_switch_provider(),
                                ChatAction::ListProviderModels(name, config) => {
                                    self.spawn_provider_models(name, config)
                                }
                            }
                        }
                    }
                }
            }
            Event::Paste(text) => {
                if let Screen::Chat(chat) = &mut self.screen {
                    chat.handle_paste(&text);
                }
            }
            _ => {}
        }
    }

    fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::ConnectResult { conn, result } => match &mut self.screen {
                Screen::Setup(setup) => {
                    setup.connecting = false;
                    match result {
                        Ok((client, store)) => {
                            if let Err(error) = self.config.save_connection(&conn) {
                                log::warn!("Failed to save connection profile: {error}");
                            }
                            self.config.set_default_connection(&conn);
                            if let Err(error) = self.config.write() {
                                log::warn!("Failed to save default connection: {error}");
                            }
                            self.screen = Screen::Chat(ChatScreen::new(
                                *conn,
                                client,
                                store,
                                Arc::clone(&self.config),
                            ));
                            self.spawn_fetch_models();
                        }
                        Err(err) => setup.error = Some(err),
                    }
                }
                Screen::Chat(chat) => {
                    if chat.on_connect_result(*conn, result) {
                        self.spawn_fetch_models();
                    }
                }
            },
            AppEvent::QueryResult(result) => {
                if let Screen::Chat(chat) = &mut self.screen {
                    chat.on_query_result(result);
                }
            }
            AppEvent::ExplainResult(result) => {
                if let Screen::Chat(chat) = &mut self.screen {
                    chat.on_explain_result(result);
                }
            }
            AppEvent::AnalyzeResult(result) => {
                if let Screen::Chat(chat) = &mut self.screen {
                    chat.on_analyze_result(result);
                }
            }
            AppEvent::AnalyzeProgress(progress) => {
                if let Screen::Chat(chat) = &mut self.screen {
                    chat.on_analyze_progress(progress);
                }
            }
            AppEvent::RelKindResult(result) => {
                if let Screen::Chat(chat) = &mut self.screen {
                    chat.on_relkind_result(result);
                }
            }
            AppEvent::ProviderReady(ready) => {
                if let Screen::Chat(chat) = &mut self.screen {
                    chat.on_provider_ready(ready);
                }
            }
            AppEvent::ProviderModelsResult(name, result) => {
                if let Screen::Chat(chat) = &mut self.screen {
                    chat.on_provider_models(&name, result);
                }
            }
            AppEvent::ModelsResult(result) => {
                if let Screen::Chat(chat) = &mut self.screen {
                    chat.on_models_result(result.unwrap_or_default());
                }
            }
        }
    }

    fn spawn_connect(&self, conn: Connection) {
        let tx = self.events_tx.clone();
        tokio::spawn(async move {
            let result = async {
                let client = db::connect(&conn).await.map_err(|e| e.to_string())?;
                let store = Store::open(&conn).await.map_err(|e| e.to_string())?;
                match db::index_stats(&client).await {
                    Ok(stats) => {
                        if let Err(error) = store.record_index_stats(&stats).await {
                            log::warn!("Failed to store index statistics: {error}");
                        }
                    }
                    Err(error) => log::warn!("Failed to read index statistics: {error}"),
                }
                Ok((client, store))
            }
            .await;
            let _ = tx.send(AppEvent::ConnectResult {
                conn: Box::new(conn),
                result,
            });
        });
    }

    fn spawn_query(&self, sql: String) {
        let tx = self.events_tx.clone();
        if let Screen::Chat(chat) = &self.screen {
            let client = Arc::clone(&chat.client);
            tokio::spawn(async move {
                let result = db::run_query(&client, &sql)
                    .await
                    .map_err(|e| e.to_string());
                let _ = tx.send(AppEvent::QueryResult(result));
            });
        }
    }

    fn spawn_explain(&self, sql: String) {
        let tx = self.events_tx.clone();
        if let Screen::Chat(chat) = &self.screen {
            let client = Arc::clone(&chat.client);
            tokio::spawn(async move {
                let result = db::explain(&client, &sql, None)
                    .await
                    .map_err(|e| e.to_string());
                let _ = tx.send(AppEvent::ExplainResult(result));
            });
        }
    }

    fn spawn_analyze(&self, sql: String) {
        let tx = self.events_tx.clone();
        let config = Arc::clone(&self.config);
        if let Screen::Chat(chat) = &self.screen {
            let client = Arc::clone(&chat.client);
            let model = chat.current_model();
            tokio::spawn(async move {
                let progress_tx = tx.clone();
                let result = async {
                    let provider_config = config
                        .select_provider()
                        .map_err(agent::AgentError::Config)?;
                    let provider = agent::build(&provider_config)?;
                    let model = model.as_deref();
                    log::info!(model = model.unwrap_or_else(|| provider.default_model()); "Selected model");
                    agent::analyze_query_streaming(
                        client,
                        &sql,
                        provider.as_ref(),
                        model,
                        move |progress| {
                            let _ = progress_tx.send(AppEvent::AnalyzeProgress(progress));
                        },
                    )
                    .await
                }
                .await
                .map_err(|e| e.to_string());
                let _ = tx.send(AppEvent::AnalyzeResult(result));
            });
        }
    }

    fn spawn_relkind(&self, schema: String, relation: String) {
        let tx = self.events_tx.clone();
        if let Screen::Chat(chat) = &self.screen {
            let client = Arc::clone(&chat.client);
            tokio::spawn(async move {
                let result = db::get_relation_kind(&client, &schema, &relation)
                    .await
                    .map_err(|e| e.to_string());
                let _ = tx.send(AppEvent::RelKindResult(result));
            });
        }
    }

    fn spawn_switch_provider(&self) {
        let tx = self.events_tx.clone();
        let config = Arc::clone(&self.config);
        tokio::spawn(async move {
            let (ready, result) = match config.select_provider() {
                Ok(provider_config) if agent::is_provider_ready(&provider_config).await => (
                    true,
                    agent::list_models(&provider_config)
                        .await
                        .map_err(|e| e.to_string()),
                ),
                _ => (false, Ok(Vec::new())),
            };
            let _ = tx.send(AppEvent::ProviderReady(ready));
            let _ = tx.send(AppEvent::ModelsResult(result));
        });
    }

    fn spawn_provider_models(&self, name: String, config: config::ProviderConfig) {
        let tx = self.events_tx.clone();
        tokio::spawn(async move {
            let result = agent::list_models(&config).await.map_err(|e| e.to_string());
            let _ = tx.send(AppEvent::ProviderModelsResult(name, result));
        });
    }

    fn spawn_fetch_models(&self) {
        let tx = self.events_tx.clone();
        let config = Arc::clone(&self.config);
        tokio::spawn(async move {
            let result = match config.select_provider() {
                Ok(provider_config) => agent::list_models(&provider_config)
                    .await
                    .map_err(|e| e.to_string()),
                Err(err) => Err(err),
            };
            let _ = tx.send(AppEvent::ModelsResult(result));
        });
    }
}

/// Ctrl+C or Ctrl+D always exits, regardless of screen/focus.
fn is_quit(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
}
