//! Reusable form and menu widgets shared by the setup and settings screens.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme;

fn framed(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT_DIM))
        .title(format!(" {title} "))
        .title_style(
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
}

/// A vertical list of labelled text fields with a single focused row.
pub struct Form {
    title: String,
    labels: Vec<&'static str>,
    pub values: Vec<String>,
    pub focus: usize,
    masked: Vec<usize>,
    choices: Vec<(usize, Vec<String>)>,
    drill: Vec<usize>,
}

impl Form {
    pub fn new(title: &str, labels: &[&'static str], values: Vec<String>) -> Self {
        debug_assert_eq!(labels.len(), values.len());
        Self {
            title: title.to_owned(),
            labels: labels.to_vec(),
            values,
            focus: 0,
            masked: Vec::new(),
            choices: Vec::new(),
            drill: Vec::new(),
        }
    }

    /// Mark the field as opening a sub-list on Enter.
    pub fn drill(mut self, index: usize) -> Self {
        self.drill.push(index);
        self
    }

    pub fn focus_is_drill(&self) -> bool {
        self.drill.contains(&self.focus)
    }

    /// Display the field as bullets.
    pub fn masked(mut self, index: usize) -> Self {
        self.masked.push(index);
        self
    }

    /// Restrict the field to `options`, cycled with Left/Right.
    pub fn choice(mut self, index: usize, options: Vec<String>) -> Self {
        self.choices.push((index, options));
        self
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn height(&self) -> u16 {
        self.values.len() as u16 + 2
    }

    /// Handles navigation and editing keys; returns true if the form changed
    /// a value.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let len = self.values.len();
        let options = self
            .choices
            .iter()
            .find(|(index, _)| *index == self.focus)
            .map(|(_, options)| options.as_slice());
        match key.code {
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % len,
            KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + len - 1) % len,
            KeyCode::Left | KeyCode::Right if options.is_some() => {
                let options = options.unwrap_or_default();
                if options.is_empty() {
                    return false;
                }
                let pos = options.iter().position(|o| *o == self.values[self.focus]);
                let count = options.len();
                let next = match (pos, key.code == KeyCode::Right) {
                    (None, _) => 0,
                    (Some(p), true) => (p + 1) % count,
                    (Some(p), false) => (p + count - 1) % count,
                };
                self.values[self.focus] = options[next].clone();
                return true;
            }
            KeyCode::Backspace if options.is_none() => {
                return self.values[self.focus].pop().is_some();
            }
            KeyCode::Char(c)
                if options.is_none() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.values[self.focus].push(c);
                return true;
            }
            _ => {}
        }
        false
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let block = framed(&self.title);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Length(1); self.values.len()])
            .split(inner);

        for (i, label) in self.labels.iter().enumerate() {
            let masked;
            let value = if self.masked.contains(&i) {
                masked = "•".repeat(self.values[i].chars().count());
                &masked
            } else {
                &self.values[i]
            };
            let drill = self.drill.contains(&i);
            draw_field(frame, rows[i], label, value, i == self.focus, drill);
        }
    }
}

fn draw_field(frame: &mut Frame, area: Rect, label: &str, value: &str, focused: bool, drill: bool) {
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
    let cursor = if focused && !drill { "▏" } else { "" };
    let suffix = if drill { "  ›" } else { "" };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{value}{cursor}"), value_style),
            Span::styled(suffix, Style::default().fg(theme::MUTED)),
        ])),
        cols[1],
    );
}

/// Returns `(first item, item count, arrow above, arrow below)` for a list of
/// `len` items in `visible` rows. The selection is centered; arrows take their
/// own rows, so the first and last items are fully shown at the list ends.
fn scroll_window(len: usize, visible: usize, selected: usize) -> (usize, usize, bool, bool) {
    if len <= visible {
        return (0, len, false, false);
    }
    if visible < 3 {
        let start = selected.saturating_sub(visible / 2).min(len - visible);
        return (start, visible, false, false);
    }
    let mid_count = visible - 2;
    let mid_start = selected.saturating_sub(mid_count / 2);
    if mid_start <= 1 {
        (0, visible - 1, false, true)
    } else if mid_start + mid_count >= len - 1 {
        (len - (visible - 1), visible - 1, true, false)
    } else {
        (mid_start, mid_count, true, true)
    }
}

/// A framed list of entries with one highlighted row.
pub fn draw_menu(frame: &mut Frame, area: Rect, title: &str, items: &[String], selected: usize) {
    let block = framed(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("(empty)", Style::default().fg(theme::MUTED))),
            inner,
        );
        return;
    }
    let visible = usize::from(inner.height);
    if visible == 0 {
        return;
    }
    let (start, count, above, below) = scroll_window(items.len(), visible, selected);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(1); visible])
        .split(inner);
    let arrow = |frame: &mut Frame, row: Rect, symbol: &str| {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("{symbol} "),
                Style::default().fg(theme::MUTED),
            )),
            row,
        );
    };
    if above {
        arrow(frame, rows[0], "↑");
    }
    if below {
        arrow(frame, rows[visible - 1], "↓");
    }
    let first_row = usize::from(above);
    for (row, (i, item)) in rows[first_row..]
        .iter()
        .zip(items.iter().enumerate().skip(start).take(count))
    {
        let (marker, style) = if i == selected {
            (
                "▸ ",
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            ("  ", Style::default().fg(theme::TEXT))
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{marker}{item}"), style)),
            *row,
        );
    }
}
