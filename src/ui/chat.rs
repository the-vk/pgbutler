//! Main "chat" screen: a GitHub Copilot CLI-style scrollback of
//! message bubbles with a bottom input box, used to run SQL against the
//! connected database.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use tokio_postgres::Client;

use crate::agent::{AnalysisProgress, Response};
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
    input_cursor: usize,
    pub busy: bool,
    analysis_preview: Option<String>,
    usage_summary: Option<String>,
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
            input_cursor: 0,
            busy: false,
            analysis_preview: None,
            usage_summary: None,
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
                self.input.insert(self.input_cursor, c);
                self.input_cursor += c.len_utf8();
            }
            KeyCode::Backspace => {
                if self.input_cursor > 0 {
                    let previous = previous_char_boundary(&self.input, self.input_cursor);
                    self.input.drain(previous..self.input_cursor);
                    self.input_cursor = previous;
                }
            }
            KeyCode::Delete => {
                if self.input_cursor < self.input.len() {
                    let next = next_char_boundary(&self.input, self.input_cursor);
                    self.input.drain(self.input_cursor..next);
                }
            }
            KeyCode::Left => {
                self.input_cursor = previous_char_boundary(&self.input, self.input_cursor);
            }
            KeyCode::Right => {
                self.input_cursor = next_char_boundary(&self.input, self.input_cursor);
            }
            KeyCode::Home => {
                self.input_cursor = self.input[..self.input_cursor]
                    .rfind('\n')
                    .map_or(0, |index| index + 1);
            }
            KeyCode::End => {
                self.input_cursor = self.input[self.input_cursor..]
                    .find('\n')
                    .map_or(self.input.len(), |index| self.input_cursor + index);
            }
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Enter => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.input.insert(self.input_cursor, '\n');
                    self.input_cursor += 1;
                    return None;
                }
                let Some(text) = take_query(&mut self.input, &mut self.input_cursor) else {
                    return None;
                };
                if let Some(action) = self.parse_chat_command(&text) {
                    self.busy = true;
                    return Some(action);
                };
            }
            _ => {}
        }
        None
    }

    pub fn handle_paste(&mut self, text: &str) {
        if self.busy {
            return;
        }
        insert_at_cursor(&mut self.input, &mut self.input_cursor, text);
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
        self.usage_summary = None;
        self.analysis_preview = Some(String::new());
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

    pub fn on_analyze_result(&mut self, result: Result<Response, String>) {
        self.busy = false;
        self.analysis_preview = None;
        match result {
            Ok(response) => {
                self.usage_summary = Some(format!(
                    "in: {} out: {} req: {}",
                    response.input_tokens, response.output_tokens, response.model_requests
                ));
                self.messages.push(Message {
                    role: Role::Assistant,
                    content: response.answer,
                });
            }
            Err(err) => self.messages.push(Message {
                role: Role::Error,
                content: err,
            }),
        }
    }

    pub fn on_analyze_progress(&mut self, progress: AnalysisProgress) {
        if let Some(preview) = &mut self.analysis_preview {
            match progress {
                AnalysisProgress::Text(text) => preview.push_str(&text),
                AnalysisProgress::Reset => preview.clear(),
            }
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

        let input_line_ranges = line_ranges(&self.input);
        let query_line_count = input_line_ranges.len() as u16;
        let max_query_lines = area.height.saturating_sub(7).max(1);
        let input_height = query_line_count.min(max_query_lines) + 2;
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(3),
                Constraint::Length(input_height),
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

        if let Some(preview) = &self.analysis_preview {
            self.draw_analysis_preview(frame, chunks[1], preview);
        }

        let input_title = if self.busy { " running… " } else { " query " };
        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::ACCENT_DIM))
            .title(input_title)
            .title_style(Style::default().fg(theme::ACCENT));
        let cursor_line = input_line_ranges
            .iter()
            .position(|(_, line_end, break_end)| {
                self.input_cursor <= *line_end
                    || (self.input_cursor > *line_end && self.input_cursor < *break_end)
            })
            .unwrap_or(input_line_ranges.len() - 1);
        let input_lines: Vec<Line> = input_line_ranges
            .iter()
            .enumerate()
            .map(|(line_index, (line_start, line_end, _))| {
                let text = &self.input[*line_start..*line_end];
                let cursor_on_line = line_index == cursor_line;
                let cursor_offset = self
                    .input_cursor
                    .saturating_sub(*line_start)
                    .min(text.len());
                let prefix = if line_index == 0 { "> " } else { "  " };
                let mut spans = vec![Span::styled(prefix, Style::default().fg(theme::ACCENT))];

                if cursor_on_line {
                    spans.push(Span::styled(
                        &text[..cursor_offset],
                        Style::default().fg(theme::TEXT),
                    ));
                    spans.push(Span::styled("▏", Style::default().fg(theme::ACCENT)));
                    spans.push(Span::styled(
                        &text[cursor_offset..],
                        Style::default().fg(theme::TEXT),
                    ));
                } else {
                    spans.push(Span::styled(text, Style::default().fg(theme::TEXT)));
                }
                Line::from(spans)
            })
            .collect();
        let input = Paragraph::new(input_lines).block(input_block);
        frame.render_widget(input, chunks[2]);

        self.draw_token_usage(frame, &chunks[2], input_title);
    }

    fn draw_analysis_preview(&self, frame: &mut Frame<'_>, area: Rect, preview: &str) {
        let width = (area.width / 3).max(1).min(area.width);
        let panel_area = Rect::new(area.x + area.width - width, area.y, width, area.height);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::ACCENT))
            .style(Style::default().fg(theme::TEXT).bg(theme::BG))
            .title(" generated text ")
            .title_style(Style::default().fg(theme::ACCENT));
        let inner = block.inner(panel_area);
        let paragraph = Paragraph::new(preview).wrap(Wrap { trim: false });
        let wrap_width = inner.width.max(1) as usize;
        let line_count = preview
            .lines()
            .map(|line| line.chars().count().max(1).div_ceil(wrap_width))
            .sum::<usize>();
        let scroll = line_count
            .saturating_sub(inner.height as usize)
            .min(u16::MAX as usize) as u16;

        frame.render_widget(Clear, panel_area);
        frame.render_widget(paragraph.scroll((scroll, 0)).block(block), panel_area);
    }

    fn draw_token_usage(&self, frame: &mut Frame<'_>, render_area: &Rect, input_title: &str) {
        if let Some(usage) = &self.usage_summary {
            let overlay_text = format!(" {usage} ");
            let overlay_width = overlay_text.chars().count() as u16;
            let required_width = input_title.chars().count() as u16 + overlay_width + 2;
            if render_area.width >= required_width + 2 {
                let overlay = Rect::new(
                    render_area.x + render_area.width - overlay_width - 1,
                    render_area.y,
                    overlay_width,
                    1,
                );
                frame.render_widget(
                    Paragraph::new(overlay_text)
                        .style(Style::default().fg(theme::MUTED).bg(theme::BG)),
                    overlay,
                );
            }
        }
    }
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn insert_at_cursor(input: &mut String, cursor: &mut usize, text: &str) {
    input.insert_str(*cursor, text);
    *cursor += text.len();
}

fn line_ranges(input: &str) -> Vec<(usize, usize, usize)> {
    let bytes = input.as_bytes();
    let mut ranges = Vec::new();
    let mut line_start = 0;
    let mut index = 0;

    while index < bytes.len() {
        if matches!(bytes[index], b'\r' | b'\n') {
            let line_end = index;
            let is_carriage_return = bytes[index] == b'\r';
            index += 1;
            if is_carriage_return && bytes.get(index) == Some(&b'\n') {
                index += 1;
            }
            ranges.push((line_start, line_end, index));
            line_start = index;
        } else {
            index += 1;
        }
    }

    ranges.push((line_start, input.len(), input.len()));
    ranges
}

fn take_query(input: &mut String, cursor: &mut usize) -> Option<String> {
    let text = std::mem::take(input);
    *cursor = 0;
    (!text.trim().is_empty()).then_some(text)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(offset, _)| cursor + offset)
}

#[cfg(test)]
mod tests {
    use super::{insert_at_cursor, line_ranges, take_query};

    #[test]
    fn paste_preserves_cr_and_lf_at_cursor() {
        let mut input = "SELECT  FROM item".to_string();
        let mut cursor = "SELECT ".len();
        let pasted = "*\r\nWHERE id = 1\rAND active\n";

        insert_at_cursor(&mut input, &mut cursor, pasted);

        assert_eq!(input, "SELECT *\r\nWHERE id = 1\rAND active\n FROM item");
        assert_eq!(cursor, "SELECT ".len() + pasted.len());
    }

    #[test]
    fn submission_preserves_trailing_line_endings() {
        let mut input = "SELECT 1\r\n".to_string();
        let mut cursor = input.len();

        assert_eq!(
            take_query(&mut input, &mut cursor),
            Some("SELECT 1\r\n".to_string())
        );
        assert!(input.is_empty());
        assert_eq!(cursor, 0);
    }

    #[test]
    fn line_ranges_recognize_lf_crlf_and_cr() {
        let input = "one\r\ntwo\rthree\nfour";
        let ranges = line_ranges(input);
        let lines: Vec<&str> = ranges
            .iter()
            .map(|(start, end, _)| &input[*start..*end])
            .collect();

        assert_eq!(lines, ["one", "two", "three", "four"]);
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
