//! Connection setup wizard: a small multi-field form used both for first-run
//! onboarding and for editing/retrying an existing profile.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::config::{Connection, SslMode};
use crate::theme;
use crate::ui::draw_banner;
use crate::ui::widgets::Form;

const FIELD_LABELS: [&str; 11] = [
    "Connection name",
    "Host",
    "Port",
    "Database",
    "User",
    "Password (optional)",
    "SSL mode",
    "Root CA cert (sslrootcert)",
    "Client cert (sslcert)",
    "Client key (sslkey)",
    "Collect index stats",
];

const PASSWORD_FIELD: usize = 5;
const SSLMODE_FIELD: usize = 6;
const COLLECT_FIELD: usize = 10;

/// Build the connection details form pre-filled from `conn`.
pub fn connection_form(conn: Connection) -> Form {
    let modes = SslMode::ALL.iter().map(ToString::to_string).collect();
    Form::new(
        "Connection details",
        &FIELD_LABELS,
        vec![
            conn.name,
            conn.host,
            conn.port.to_string(),
            conn.catalog,
            conn.login,
            conn.password,
            conn.sslmode.to_string(),
            conn.sslrootcert.unwrap_or_default(),
            conn.sslcert.unwrap_or_default(),
            conn.sslkey.unwrap_or_default(),
            conn.collect_index_stats.to_string(),
        ],
    )
    .masked(PASSWORD_FIELD)
    .choice(SSLMODE_FIELD, modes)
    .choice(COLLECT_FIELD, vec!["false".to_owned(), "true".to_owned()])
}

/// Build a `Connection` from the form values, without validating it.
pub fn connection_from_form(form: &Form) -> Result<Connection, String> {
    let values = &form.values;
    let port: u16 = values[2]
        .trim()
        .parse()
        .map_err(|_| format!("invalid port '{}': must be a number 1-65535", values[2]))?;
    let sslmode: SslMode = values[SSLMODE_FIELD].parse()?;
    let non_empty = |s: &str| {
        let s = s.trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    };
    Ok(Connection {
        name: values[0].trim().to_string(),
        host: values[1].trim().to_string(),
        port,
        catalog: values[3].trim().to_string(),
        login: values[4].trim().to_string(),
        password: values[PASSWORD_FIELD].clone(),
        collect_index_stats: values[COLLECT_FIELD] == "true",
        sslmode,
        sslrootcert: non_empty(&values[7]),
        sslcert: non_empty(&values[8]),
        sslkey: non_empty(&values[9]),
    })
}

pub struct SetupScreen {
    form: Form,
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
            form: connection_form(conn),
            connecting: false,
            error: None,
            autoconnect: false,
        }
    }

    /// Snapshot of the draft connection, used by the app to kick off an
    /// automatic connect attempt on startup.
    pub fn draft(&self) -> Connection {
        // Autoconnect only happens right after `from_connection`, so this is
        // always a valid, previously-saved profile.
        connection_from_form(&self.form)
            .unwrap_or_else(|_| Connection::with_defaults(&self.form.values[0]))
    }

    /// Handle a key press. Returns `Some(Connection)` when the user has
    /// submitted the form and it passed validation, meaning the caller
    /// should attempt to connect.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Connection> {
        if self.connecting {
            return None; // ignore input while a connection attempt is in flight
        }
        if key.code == KeyCode::Enter {
            if self.form.focus + 1 < self.form.len() {
                self.form.focus += 1;
            } else {
                return self.submit();
            }
        } else {
            self.form.handle_key(key);
        }
        None
    }

    fn submit(&mut self) -> Option<Connection> {
        match connection_from_form(&self.form).and_then(|conn| conn.validate().map(|()| conn)) {
            Ok(conn) => {
                self.error = None;
                Some(conn)
            }
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
                Constraint::Length(self.form.height()),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(area);

        draw_banner(frame, chunks[0], "Set up a PostgreSQL connection");
        self.form.draw(frame, chunks[1]);

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
                " Tab/Shift+Tab to move, ←/→ to change SSL mode, Enter to confirm, Ctrl+C to quit ",
                Style::default().fg(theme::MUTED),
            ))
        };
        frame.render_widget(Paragraph::new(status), chunks[2]);
    }
}
