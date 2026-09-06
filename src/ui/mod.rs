pub mod chat;
pub mod setup;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme;

/// Draws the shared top banner used on every screen, Copilot-CLI style:
/// a bold accent title plus a muted subtitle line.
pub fn draw_banner(frame: &mut Frame, area: Rect, subtitle: &str) {
    let lines = vec![Line::from(vec![
        Span::styled(
            " pgbutler ",
            Style::default()
                .fg(theme::BG)
                .bg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(subtitle, Style::default().fg(theme::MUTED)),
    ])];
    let banner = Paragraph::new(lines).block(Block::default().borders(Borders::NONE));
    frame.render_widget(banner, area);
}
