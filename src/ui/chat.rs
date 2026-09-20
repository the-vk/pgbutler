//! Main "chat" screen: a GitHub Copilot CLI-style scrollback of
//! message bubbles with a bottom input box, used to run SQL against the
//! connected database.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tokio_postgres::Client;

use crate::config::Connection;
use crate::db::{QueryOutcome, RelKind};
use crate::theme;
use crate::ui::draw_banner;

pub enum Role {
    User,
    Assistant,
    System,
    Error,
}

pub struct Message {
    pub role: Role,
    pub content: String,
}

/// Action to execute in the background when the user submits input.
pub enum ChatAction {
    Query(String),
    Explain(String),
    Analyze(String),
    RelKind(String, String),
}

pub struct ChatScreen {
    pub conn: Connection,
    pub client: Arc<Client>,
    pub messages: Vec<Message>,
    pub input: String,
    pub busy: bool,
    scroll: u16,
    /// Models reported by the local Ollama daemon, fetched on screen start.
    pub models: Vec<String>,
    /// Index into `models` of the model currently in use, if any are available.
    pub selected_model: Option<usize>,
}

impl ChatScreen {
    pub fn new(conn: Connection, client: Client) -> Self {
        let mut messages = Vec::new();
        messages.push(Message {
            role: Role::System,
            content: format!(
                "Connected to '{}' as {}@{}:{}/{} (mutual TLS, verify-full). Type SQL and press Enter. /help for commands.",
                conn.name, conn.login, conn.host, conn.port, conn.catalog
            ),
        });
        Self {
            conn,
            client: Arc::new(client),
            messages,
            input: String::new(),
            busy: false,
            scroll: 0,
            models: Vec::new(),
            selected_model: None,
        }
    }

    /// The model currently selected for AI-assisted commands, if any.
    pub fn current_model(&self) -> Option<&str> {
        self.selected_model
            .and_then(|i| self.models.get(i))
            .map(String::as_str)
    }

    /// Store the models available from Ollama and select the first one.
    pub fn on_models_result(&mut self, models: Vec<String>) {
        self.selected_model = if models.is_empty() { None } else { Some(0) };
        self.models = models;
    }

    /// Cycle to the next available model, wrapping around.
    fn cycle_model(&mut self) {
        if self.models.is_empty() {
            return;
        }
        let next = self
            .selected_model
            .map_or(0, |i| (i + 1) % self.models.len());
        self.selected_model = Some(next);
    }

    /// Handle a key press. Returns `Some(action)` when a query or command should
    /// be executed against the database.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ChatAction> {
        if key.code == KeyCode::Char('m') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.cycle_model();
            return None;
        }
        if self.busy {
            return None;
        }
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.push(c);
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Enter => {
                let text = self.input.trim().to_string();
                self.input.clear();
                if text.is_empty() {
                    return None;
                }
                if let Some(action) = self.parse_chat_command(&text) {
                    self.busy = true;
                    return Some(action);
                };
            }
            _ => {}
        }
        None
    }

    fn parse_chat_command(&mut self, text: &str) -> Option<ChatAction> {
        let text_trimmed = text.trim();
        if text_trimmed.find(|ch: char| ch.is_ascii_whitespace()) == None {
            return self.match_command(text_trimmed, text_trimmed, "");
        }
        if let Some((command, rest)) = text.split_once(|ch: char| ch.is_ascii_whitespace()) {
            self.match_command(text, command, rest)
        } else {
            None
        }
    }

    fn match_command(&mut self, text: &str, command: &str, query: &str) -> Option<ChatAction> {
        match command {
            "/analyze" => self.handle_analyze_command(text, query),
            "/explain" => self.handle_explain_command(text, query),
            "/relkind" => self.handle_relkind_command(text, query),
            "/query" => Some(ChatAction::Query(query.to_owned())),
            "/help" => self.handle_help_command(text),
            "/whoami" => self.handle_whoami_command(text),
            _ => None,
        }
    }

    fn handle_analyze_command(&mut self, text: &str, query: &str) -> Option<ChatAction> {
        if query.is_empty() {
            self.messages.push(Message {
                role: Role::User,
                content: text.to_owned(),
            });
            self.messages.push(Message {
                role: Role::System,
                content: "Usage: /analyze <SQL query>".to_string(),
            });
            return None;
        }
        self.messages.push(Message {
            role: Role::User,
            content: text.to_owned(),
        });
        self.busy = true;
        Some(ChatAction::Analyze(query.to_owned()))
    }

    fn handle_explain_command(&mut self, text: &str, query: &str) -> Option<ChatAction> {
        if query.is_empty() {
            self.messages.push(Message {
                role: Role::User,
                content: text.to_owned(),
            });
            self.messages.push(Message {
                role: Role::System,
                content: "Usage: /explain <SQL query>".to_string(),
            });
            return None;
        }
        self.messages.push(Message {
            role: Role::User,
            content: text.to_owned(),
        });
        self.busy = true;
        return Some(ChatAction::Explain(query.to_owned()));
    }

    fn handle_relkind_command(&mut self, text: &str, query: &str) -> Option<ChatAction> {
        let arg = query.trim();
        if let Some((schema, relation)) = arg.split_once('.') {
            let schema = schema.trim();
            let relation = relation.trim();
            if !schema.is_empty() && !relation.is_empty() {
                self.messages.push(Message {
                    role: Role::User,
                    content: text.to_owned(),
                });
                self.busy = true;
                return Some(ChatAction::RelKind(schema.to_owned(), relation.to_owned()));
            }
        }
        self.messages.push(Message {
            role: Role::User,
            content: text.to_owned(),
        });
        self.messages.push(Message {
            role: Role::System,
            content: "Usage: /relkind <schema_name>.<relation_name>".to_string(),
        });
        None
    }

    fn handle_help_command(&mut self, text: &str) -> Option<ChatAction> {
        let reply = "Commands: /help (this message), /whoami (show connection info), /explain <query> (explain query plan), /analyze <query> (analyze query plan and schemas), /relkind <schema.relation> (get relation kind). \
                 Anything else is sent to PostgreSQL as SQL.";
        self.messages.push(Message {
            role: Role::User,
            content: text.to_owned(),
        });
        self.messages.push(Message {
            role: Role::System,
            content: reply.to_string(),
        });
        return None;
    }

    fn handle_whoami_command(&mut self, text: &str) -> Option<ChatAction> {
        let reply = format!(
            "{}@{}:{}/{} — verify-full mTLS",
            self.conn.login, self.conn.host, self.conn.port, self.conn.catalog
        );
        self.messages.push(Message {
            role: Role::User,
            content: text.to_owned(),
        });
        self.messages.push(Message {
            role: Role::System,
            content: reply.to_string(),
        });
        return None;
    }

    pub fn on_query_result(&mut self, result: Result<QueryOutcome, String>) {
        self.busy = false;
        match result {
            Ok(outcome) => self.messages.push(Message {
                role: Role::Assistant,
                content: format_outcome(&outcome),
            }),
            Err(err) => self.messages.push(Message {
                role: Role::Error,
                content: err,
            }),
        }
    }

    pub fn on_explain_result(&mut self, result: Result<String, String>) {
        self.busy = false;
        match result {
            Ok(output) => self.messages.push(Message {
                role: Role::Assistant,
                content: output,
            }),
            Err(err) => self.messages.push(Message {
                role: Role::Error,
                content: err,
            }),
        }
    }

    pub fn on_analyze_result(&mut self, result: Result<String, String>) {
        self.busy = false;
        match result {
            Ok(output) => self.messages.push(Message {
                role: Role::Assistant,
                content: output,
            }),
            Err(err) => self.messages.push(Message {
                role: Role::Error,
                content: err,
            }),
        }
    }

    pub fn on_relkind_result(&mut self, result: Result<RelKind, String>) {
        self.busy = false;
        match result {
            Ok(rel_kind) => self.messages.push(Message {
                role: Role::Assistant,
                content: format!("{rel_kind:?}"),
            }),
            Err(err) => self.messages.push(Message {
                role: Role::Error,
                content: err,
            }),
        }
    }

    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(Block::default().style(Style::default().bg(theme::BG)), area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(area);

        let model_label = self.current_model().unwrap_or("<no models available>");
        draw_banner(
            frame,
            chunks[0],
            &format!(
                "{} — {}@{} — model: {} (Ctrl+m to switch)",
                self.conn.name, self.conn.login, self.conn.host, model_label
            ),
        );

        let mut lines: Vec<Line> = Vec::new();
        for msg in &self.messages {
            let (prefix, style) = match msg.role {
                Role::User => ("you  ▸ ", Style::default().fg(theme::USER_BUBBLE)),
                Role::Assistant => ("sql  ▸ ", Style::default().fg(theme::TEXT)),
                Role::System => ("info ▸ ", Style::default().fg(theme::ACCENT)),
                Role::Error => ("error▸ ", Style::default().fg(theme::ERROR)),
            };
            for (i, line) in msg.content.lines().enumerate() {
                let prefix = if i == 0 { prefix } else { "       " };
                lines.push(Line::from(vec![
                    Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
                    Span::styled(line.to_string(), style),
                ]));
            }
            lines.push(Line::default());
        }
        let log = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0))
            .block(Block::default().borders(Borders::NONE));
        frame.render_widget(log, chunks[1]);

        let input_title = if self.busy { " running… " } else { " query " };
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::ACCENT_DIM))
            .title(input_title)
            .title_style(Style::default().fg(theme::ACCENT));
        let input = Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(theme::ACCENT)),
            Span::styled(self.input.as_str(), Style::default().fg(theme::TEXT)),
            Span::styled("▏", Style::default().fg(theme::ACCENT)),
        ]))
        .block(input_block);
        frame.render_widget(input, chunks[2]);
    }
}

fn format_outcome(outcome: &QueryOutcome) -> String {
    if outcome.rows.is_empty() && !outcome.columns.is_empty() {
        return "0 rows".to_string();
    }
    if outcome.columns.is_empty() {
        return if outcome.statuses.is_empty() {
            "OK".to_string()
        } else {
            outcome.statuses.join(", ")
        };
    }

    let widths: Vec<usize> = outcome
        .columns
        .iter()
        .enumerate()
        .map(|(i, col)| {
            outcome
                .rows
                .iter()
                .map(|r| r[i].len())
                .chain(std::iter::once(col.len()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    let mut out = String::new();
    let header: Vec<String> = outcome
        .columns
        .iter()
        .zip(&widths)
        .map(|(c, w)| format!("{c:<w$}"))
        .collect();
    out.push_str(&header.join(" | "));
    out.push('\n');
    out.push_str(
        &widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("-+-"),
    );
    for row in &outcome.rows {
        out.push('\n');
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(v, w)| format!("{v:<w$}"))
            .collect();
        out.push_str(&cells.join(" | "));
    }
    if !outcome.statuses.is_empty() {
        out.push('\n');
        out.push_str(&outcome.statuses.join(", "));
    }
    out
}
