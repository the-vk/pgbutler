//! Connection setup wizard: a small multi-field form used both for first-run
//! onboarding and for editing/retrying an existing profile.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::config::Connection;
use crate::theme;
use crate::ui::draw_banner;

const FIELD_LABELS: [&str; 8] = [
    "Connection name",
    "Host",
    "Port",
    "Database",
    "User",
    "Root CA cert (sslrootcert)",
    "Client cert (sslcert)",
    "Client key (sslkey)",
];

pub struct SetupScreen {
    values: [String; 8],
    focus: usize,
    pub connecting: bool,
    pub error: Option<String>,
    /// Set by `App` right after construction when a saved profile should be
    /// tried immediately, without waiting for user input.
    pub autoconnect: bool,
}

impl SetupScreen {
    /// Fresh wizard pre-filled with sane defaults, for first-run onboarding.
    pub fn new() -> Self {
        Self::from_connection(Connection::with_defaults("default"))
    }

    /// Wizard pre-filled from an existing saved connection (used for
    /// automatic reconnect attempts and manual edits).
    pub fn from_connection(conn: Connection) -> Self {
        Self {
            values: [
                conn.name,
                conn.host,
                conn.port.to_string(),
                conn.catalog,
                conn.login,
                conn.sslrootcert,
                conn.sslcert,
                conn.sslkey,
            ],
            focus: 0,
            connecting: false,
            error: None,
            autoconnect: false,
        }
    }

    /// Build the `Connection` currently held in the draft, without applying
    /// any validation yet.
    fn draft_connection(&self) -> Result<Connection, String> {
        let port: u16 = self.values[2].trim().parse().map_err(|_| {
            format!(
                "invalid port '{}': must be a number 1-65535",
                self.values[2]
            )
        })?;
        Ok(Connection {
            name: self.values[0].trim().to_string(),
            host: self.values[1].trim().to_string(),
            port,
            catalog: self.values[3].trim().to_string(),
            login: self.values[4].trim().to_string(),
            sslrootcert: self.values[5].trim().to_string(),
            sslcert: self.values[6].trim().to_string(),
            sslkey: self.values[7].trim().to_string(),
        })
    }

    /// Snapshot of the draft connection, used by the app to kick off an
    /// automatic connect attempt on startup.
    pub fn draft(&self) -> Connection {
        // Autoconnect only happens right after `from_connection`, so this is
        // always a valid, previously-saved profile.
        self.draft_connection()
            .unwrap_or_else(|_| Connection::with_defaults(&self.values[0]))
    }

    /// Handle a key press. Returns `Some(Connection)` when the user has
    /// submitted the form and it passed validation, meaning the caller
    /// should attempt to connect.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Connection> {
        if self.connecting {
            return None; // ignore input while a connection attempt is in flight
        }
        match key.code {
            KeyCode::Tab | KeyCode::Down => {
                self.focus = (self.focus + 1) % self.values.len();
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.focus = (self.focus + self.values.len() - 1) % self.values.len();
            }
            KeyCode::Backspace => {
                self.values[self.focus].pop();
            }
            KeyCode::Char(c) => {
                self.values[self.focus].push(c);
            }
            KeyCode::Enter => {
                if self.focus + 1 < self.values.len() {
                    self.focus += 1;
                } else {
                    return self.submit();
                }
            }
            _ => {}
        }
        None
    }

    fn submit(&mut self) -> Option<Connection> {
        match self.draft_connection() {
            Ok(conn) => match conn.validate() {
                Ok(()) => {
                    self.error = None;
                    Some(conn)
                }
                Err(err) => {
                    self.error = Some(err);
                    None
                }
            },
            Err(err) => {
                self.error = Some(err);
                None
            }
        }
    }

    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(Block::default().style(Style::default().bg(theme::BG)), area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(self.values.len() as u16 + 2),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(area);

        draw_banner(
            frame,
            chunks[0],
            "Set up a PostgreSQL connection (mutual TLS required)",
        );

        let form_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::ACCENT_DIM))
            .title(" Connection details ")
            .title_style(
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            );
        let inner = form_block.inner(chunks[1]);
        frame.render_widget(form_block, chunks[1]);

        let field_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Length(1); self.values.len()])
            .split(inner);

        for (i, label) in FIELD_LABELS.iter().enumerate() {
            self.draw_field(
                frame,
                field_rows[i],
                label,
                &self.values[i],
                i == self.focus,
            );
        }

        let status = if self.connecting {
            Line::from(Span::styled(
                " Connecting… ",
                Style::default()
                    .fg(theme::WARNING)
                    .add_modifier(Modifier::BOLD),
            ))
        } else if let Some(err) = &self.error {
            Line::from(Span::styled(
                format!(" ✗ {err}"),
                Style::default().fg(theme::ERROR),
            ))
        } else {
            Line::from(Span::styled(
                " Tab/Shift+Tab to move between fields, Enter to confirm, Ctrl+C to quit ",
                Style::default().fg(theme::MUTED),
            ))
        };
        frame.render_widget(Paragraph::new(status), chunks[2]);
    }

    fn draw_field(&self, frame: &mut Frame, area: Rect, label: &str, value: &str, focused: bool) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(1)])
            .split(area);

        let label_style = if focused {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::MUTED)
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{label:>26} "), label_style))
                .alignment(Alignment::Left),
            cols[0],
        );

        let value_style = if focused {
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::UNDERLINED)
        } else {
            Style::default().fg(theme::TEXT)
        };
        let cursor = if focused { "▏" } else { "" };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{value}{cursor}"), value_style)),
            cols[1],
        );
    }
}
