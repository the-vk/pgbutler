//! Visual theme constants, loosely modelled after the GitHub Copilot CLI's
//! dark, purple-accented look.
#![allow(dead_code)]

use ratatui::style::Color;

pub const BG: Color = Color::Rgb(13, 17, 23); // GitHub dark background
pub const PANEL_BG: Color = Color::Rgb(22, 27, 34);
pub const ACCENT: Color = Color::Rgb(163, 113, 247); // Copilot purple
pub const ACCENT_DIM: Color = Color::Rgb(110, 84, 148);
pub const TEXT: Color = Color::Rgb(201, 209, 217);
pub const MUTED: Color = Color::Rgb(125, 133, 144);
pub const SUCCESS: Color = Color::Rgb(63, 185, 80);
pub const WARNING: Color = Color::Rgb(210, 153, 34);
pub const ERROR: Color = Color::Rgb(248, 81, 73);
pub const USER_BUBBLE: Color = Color::Rgb(56, 139, 253);
