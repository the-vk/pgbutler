//! Application state machine and the terminal event loop.

use std::sync::Arc;

use color_eyre::eyre::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::{FutureExt, StreamExt};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;
use tokio_postgres::Client;

use crate::config::Connection;
use crate::db::{self, QueryOutcome};
use crate::ui::chat::{ChatAction, ChatScreen};
use crate::ui::setup::SetupScreen;

/// Top-level screen currently being shown.
pub enum Screen {
    Setup(SetupScreen),
    Chat(ChatScreen),
}

/// Events produced by background async work and fed back into the main loop.
#[allow(clippy::enum_variant_names)]
pub enum AppEvent {
    ConnectResult {
        conn: Box<Connection>,
        result: Result<Client, String>,
    },
    QueryResult(Result<QueryOutcome, String>),
    ExplainResult(Result<String, String>),
}

pub struct App {
    pub screen: Screen,
    pub should_quit: bool,
    events_tx: mpsc::UnboundedSender<AppEvent>,
    events_rx: mpsc::UnboundedReceiver<AppEvent>,
}

impl App {
    /// Build the initial app state: skip straight to the connection setup
    /// wizard if no connection profiles exist yet, otherwise open the first
    /// saved connection directly.
    pub fn new() -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let connections = crate::config::list_connections().unwrap_or_default();

        let screen = if let Some(conn) = connections.into_iter().next() {
            let mut setup = SetupScreen::from_connection(conn);
            setup.autoconnect = true;
            Screen::Setup(setup)
        } else {
            Screen::Setup(SetupScreen::new())
        };

        Self {
            screen,
            should_quit: false,
            events_tx,
            events_rx,
        }
    }

    pub async fn run(&mut self, mut terminal: DefaultTerminal) -> Result<()> {
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
        Ok(())
    }

    fn handle_terminal_event(&mut self, event: Event) {
        if let Event::Key(key) = event {
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
                            ChatAction::Explain(sql) => self.spawn_explain(sql),
                        }
                    }
                }
            }
        }
    }

    fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::ConnectResult { conn, result } => {
                if let Screen::Setup(setup) = &mut self.screen {
                    setup.connecting = false;
                    match result {
                        Ok(client) => {
                            let _ = crate::config::save_connection(&conn);
                            self.screen = Screen::Chat(ChatScreen::new(*conn, client));
                        }
                        Err(err) => setup.error = Some(err),
                    }
                }
            }
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
        }
    }

    fn spawn_connect(&self, conn: Connection) {
        let tx = self.events_tx.clone();
        tokio::spawn(async move {
            let result = db::connect(&conn).await.map_err(|e| e.to_string());
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
                let result = db::explain(&client, &sql).await.map_err(|e| e.to_string());
                let _ = tx.send(AppEvent::ExplainResult(result));
            });
        }
    }
}

/// Ctrl+C or Ctrl+D always exits, regardless of screen/focus.
fn is_quit(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
}
