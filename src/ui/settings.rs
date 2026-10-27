//! Settings screen: tree-like navigation over connections, application and
//! provider-specific configuration sections.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::config::{
    AnthropicConfig, Config, Connection, OllamaConfig, OpenAiConfig, ProviderConfig,
};
use crate::theme;
use crate::ui::draw_banner;
use crate::ui::setup::{connection_form, connection_from_form};
use crate::ui::widgets::{Form, draw_menu};

const PROVIDERS: [&str; 3] = ["ollama", "anthropic", "openai"];
const ROOT_ITEMS: [&str; 5] = [
    "Connections",
    "Application",
    "Ollama",
    "Anthropic",
    "OpenAI",
];

pub enum SettingsEvent {
    None,
    Close,
    /// Fetch the models offered by this (possibly unsaved) provider config.
    LoadModels(String, ProviderConfig),
}

#[derive(Clone)]
enum Node {
    Root,
    Connections,
    Connection(String),
    App,
    Provider(&'static str),
    /// Model list of the provider whose form is one level up.
    Models(&'static str),
}

struct Level {
    node: Node,
    selected: usize,
    /// Present for leaf sections that edit values.
    form: Option<Form>,
    models: Vec<String>,
    loading: bool,
}

impl Level {
    fn new(node: Node, form: Option<Form>) -> Self {
        Self {
            node,
            selected: 0,
            form,
            models: Vec::new(),
            loading: false,
        }
    }
}

pub struct SettingsScreen {
    config: Arc<Config>,
    path: Vec<Level>,
    error: Option<String>,
    delete_confirmation: Option<String>,
    provider_dirty: bool,
}

impl SettingsScreen {
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            config,
            path: vec![Level::new(Node::Root, None)],
            error: None,
            delete_confirmation: None,
            provider_dirty: false,
        }
    }

    /// True if the provider selection or its settings were changed.
    pub fn provider_dirty(&self) -> bool {
        self.provider_dirty
    }

    fn current(&self) -> &Level {
        self.path.last().expect("path always has a root")
    }

    fn current_mut(&mut self) -> &mut Level {
        self.path.last_mut().expect("path always has a root")
    }

    fn menu_items(&self) -> Vec<String> {
        match self.current().node {
            Node::Root => ROOT_ITEMS.iter().map(ToString::to_string).collect(),
            Node::Connections => self
                .config
                .connections()
                .into_iter()
                .map(|c| c.name)
                .collect(),
            Node::Models(_) => self.current().models.clone(),
            _ => Vec::new(),
        }
    }

    fn title(&self) -> String {
        let mut parts = vec!["Settings".to_owned()];
        for level in &self.path[1..] {
            parts.push(match &level.node {
                Node::Root => String::new(),
                Node::Connections => "Connections".to_owned(),
                Node::Connection(name) => name.clone(),
                Node::App => "Application".to_owned(),
                Node::Provider(name) => provider_title(name).to_owned(),
                Node::Models(_) => "Model".to_owned(),
            });
        }
        parts.join(" › ")
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SettingsEvent {
        if let Some(name) = self.delete_confirmation.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.delete_confirmation = None;
                    match self.config.delete_connection(&name) {
                        Ok(()) => {
                            let count = self.menu_items().len();
                            self.current_mut().selected =
                                self.current().selected.min(count.saturating_sub(1));
                            self.error = None;
                        }
                        Err(err) => {
                            self.error = Some(format!("failed to delete connection: {err}"))
                        }
                    }
                }
                KeyCode::Char('n') | KeyCode::Esc => self.delete_confirmation = None,
                _ => {}
            }
            return SettingsEvent::None;
        }
        if key.code == KeyCode::Esc {
            return self.back();
        }
        if self.current().form.is_some() {
            if key.code == KeyCode::Enter
                && self
                    .current()
                    .form
                    .as_ref()
                    .is_some_and(Form::focus_is_drill)
            {
                return self.open_models();
            }
            if let Some(form) = self.current_mut().form.as_mut()
                && form.handle_key(key)
            {
                self.error = None;
            }
            return SettingsEvent::None;
        }
        let count = self.menu_items().len();
        match key.code {
            KeyCode::Char('+') if matches!(self.current().node, Node::Connections) => {
                self.add_connection();
            }
            KeyCode::Char('-') if matches!(self.current().node, Node::Connections) => {
                if let Some(name) = self.menu_items().get(self.current().selected).cloned() {
                    self.delete_confirmation = Some(name);
                    self.error = None;
                }
            }
            KeyCode::Down if count > 0 => {
                let level = self.current_mut();
                level.selected = (level.selected + 1) % count;
            }
            KeyCode::Up if count > 0 => {
                let level = self.current_mut();
                level.selected = (level.selected + count - 1) % count;
            }
            KeyCode::Enter if matches!(self.current().node, Node::Models(_)) => self.pick_model(),
            KeyCode::Enter if count > 0 => self.enter(),
            _ => {}
        }
        SettingsEvent::None
    }

    fn add_connection(&mut self) {
        let existing: Vec<String> = self.menu_items();
        let base_name = "new-connection";
        let name = if !existing.iter().any(|name| name == base_name) {
            base_name.to_owned()
        } else {
            (2..)
                .map(|suffix| format!("{base_name}-{suffix}"))
                .find(|candidate| !existing.contains(candidate))
                .expect("an unused connection name always exists")
        };
        self.error = None;
        self.path.push(Level::new(
            Node::Connection(name.clone()),
            Some(connection_form(Connection::with_defaults(name))),
        ));
    }

    fn open_models(&mut self) -> SettingsEvent {
        let Node::Provider(name) = self.current().node else {
            return SettingsEvent::None;
        };
        let Some(form) = &self.current().form else {
            return SettingsEvent::None;
        };
        let config = match provider_from_form(name, form) {
            Ok(config) => config,
            Err(err) => {
                self.error = Some(err);
                return SettingsEvent::None;
            }
        };
        self.error = None;
        let mut level = Level::new(Node::Models(name), None);
        level.loading = true;
        self.path.push(level);
        SettingsEvent::LoadModels(name.to_owned(), config)
    }

    /// Receive the models fetched for `provider`; ignored if the picker was closed.
    pub fn on_models(&mut self, provider: &str, result: Result<Vec<String>, String>) {
        let current = self.parent_model();
        let level = self.current_mut();
        match level.node {
            Node::Models(name) if name == provider => {}
            _ => return,
        }
        level.loading = false;
        match result {
            Ok(models) => {
                level.selected = models.iter().position(|m| *m == current).unwrap_or(0);
                level.models = models;
            }
            Err(err) => self.error = Some(format!("failed to list models: {err}")),
        }
    }

    fn parent_model(&self) -> String {
        let parent = self.path.len().checked_sub(2).map(|i| &self.path[i]);
        match parent {
            Some(Level {
                node: Node::Provider(name),
                form: Some(form),
                ..
            }) => form.values[model_field(name)].clone(),
            _ => String::new(),
        }
    }

    fn pick_model(&mut self) {
        let level = self.current();
        let Node::Models(name) = level.node else {
            return;
        };
        let Some(model) = level.models.get(level.selected).cloned() else {
            return;
        };
        self.path.pop();
        if let Some(form) = self.current_mut().form.as_mut() {
            form.values[model_field(name)] = model;
        }
        self.error = None;
    }

    fn enter(&mut self) {
        let selected = self.current().selected;
        let node = match self.current().node {
            Node::Root => match selected {
                0 => Node::Connections,
                1 => Node::App,
                n => Node::Provider(PROVIDERS[n - 2]),
            },
            Node::Connections => match self.menu_items().into_iter().nth(selected) {
                Some(name) => Node::Connection(name),
                None => return,
            },
            _ => return,
        };
        let form = match self.build_form(&node) {
            Ok(form) => form,
            Err(err) => {
                self.error = Some(err);
                return;
            }
        };
        self.error = None;
        self.path.push(Level::new(node, form));
    }

    fn build_form(&self, node: &Node) -> Result<Option<Form>, String> {
        Ok(match node {
            Node::Root | Node::Connections | Node::Models(_) => None,
            Node::Connection(name) => {
                let conn = self
                    .config
                    .connections()
                    .into_iter()
                    .find(|c| &c.name == name)
                    .ok_or_else(|| format!("connection '{name}' not found"))?;
                Some(connection_form(conn).readonly(0))
            }
            Node::App => Some(
                Form::new(
                    "Application",
                    &["AI provider"],
                    vec![self.config.provider_name()],
                )
                .choice(0, PROVIDERS.iter().map(ToString::to_string).collect()),
            ),
            Node::Provider(name) => {
                let title = provider_title(name);
                Some(match self.config.provider_config(name)? {
                    ProviderConfig::Ollama(c) => Form::new(
                        title,
                        &["Base URL", "Model", "Context size (num_ctx)"],
                        vec![
                            c.base_url.unwrap_or_default(),
                            c.model.unwrap_or_default(),
                            c.num_ctx.map(|n| n.to_string()).unwrap_or_default(),
                        ],
                    )
                    .drill(1),
                    ProviderConfig::Anthropic(c) => api_form(title, c.api_key, c.base_url, c.model),
                    ProviderConfig::OpenAi(c) => api_form(title, c.api_key, c.base_url, c.model),
                })
            }
        })
    }

    /// Apply the current leaf form (if any) and go up one level. A failed
    /// apply keeps the user in place; pressing Esc again discards the edits.
    fn back(&mut self) -> SettingsEvent {
        if self.path.len() == 1 {
            return SettingsEvent::Close;
        }
        if self.current().form.is_some() && self.error.is_none() {
            if let Err(err) = self.apply() {
                self.error = Some(format!("{err} (Esc again to discard)"));
                return SettingsEvent::None;
            }
        }
        self.error = None;
        self.path.pop();
        SettingsEvent::None
    }

    fn apply(&mut self) -> Result<(), String> {
        let level = self.current();
        let Some(form) = &level.form else {
            return Ok(());
        };
        match &level.node {
            Node::Connection(_) => {
                let conn = connection_from_form(form)?;
                conn.validate()?;
                self.config
                    .save_connection(&conn)
                    .map_err(|e| e.to_string())?;
            }
            Node::App => {
                self.config.set_provider(&form.values[0])?;
                self.provider_dirty = true;
            }
            Node::Provider(name) => {
                let config = provider_from_form(name, form)?;
                self.config.set_provider_config(config);
                self.provider_dirty = true;
            }
            Node::Root | Node::Connections | Node::Models(_) => {}
        }
        Ok(())
    }

    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(Block::default().style(Style::default().bg(theme::BG)), area);

        let level = self.current();
        let body_height = match &level.form {
            Some(form) => form.height(),
            None => (self.menu_items().len().max(1) as u16 + 2).min(area.height.saturating_sub(5)),
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(body_height),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(area);

        draw_banner(frame, chunks[0], "Settings");
        let title = self.title();
        match &level.form {
            Some(form) => form.draw(frame, chunks[1]),
            None => {
                let title = if level.loading {
                    format!("{title} (loading…)")
                } else {
                    title
                };
                draw_menu(frame, chunks[1], &title, &self.menu_items(), level.selected)
            }
        }

        let status = match &self.error {
            Some(err) => Span::styled(format!(" ✗ {err}"), Style::default().fg(theme::ERROR)),
            None => Span::styled(
                if level.form.is_some() {
                    " Tab/Shift+Tab to move, ←/→ to change choices, Enter on › to pick from a list, Esc to save and go back "
                } else if matches!(level.node, Node::Models(_)) {
                    " ↑/↓ to select, Enter to confirm, Esc to cancel "
                } else if matches!(level.node, Node::Connections) {
                    " ↑/↓ to select, Enter to open, + to add, - to delete, Esc to go back "
                } else {
                    " ↑/↓ to select, Enter to open, Esc to go back "
                },
                Style::default().fg(theme::MUTED),
            ),
        };
        frame.render_widget(Paragraph::new(Line::from(status)), chunks[2]);

        if let Some(name) = &self.delete_confirmation {
            let popup_rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(40),
                    Constraint::Length(4),
                    Constraint::Percentage(40),
                ])
                .split(area);
            let popup_columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(20),
                    Constraint::Percentage(60),
                    Constraint::Percentage(20),
                ])
                .split(popup_rows[1]);
            let popup = popup_columns[1];
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(format!(
                    "Delete connection '{name}'?\ny/Enter: confirm   n/Esc: cancel"
                ))
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Confirm deletion "),
                ),
                popup,
            );
        }
    }
}

fn provider_title(name: &str) -> &'static str {
    match name {
        "ollama" => "Ollama",
        "anthropic" => "Anthropic",
        _ => "OpenAI",
    }
}

fn api_form(title: &str, api_key: String, base_url: Option<String>, model: Option<String>) -> Form {
    Form::new(
        title,
        &["API key", "Base URL", "Model"],
        vec![
            api_key,
            base_url.unwrap_or_default(),
            model.unwrap_or_default(),
        ],
    )
    .masked(0)
    .drill(2)
}

/// Index of the model field in a provider form.
fn model_field(provider: &str) -> usize {
    if provider == "ollama" { 1 } else { 2 }
}

fn provider_from_form(name: &str, form: &Form) -> Result<ProviderConfig, String> {
    let opt = |s: &String| {
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_owned())
    };
    let v = &form.values;
    Ok(match name {
        "ollama" => {
            let num_ctx = match v[2].trim() {
                "" => None,
                s => Some(
                    s.parse::<u64>()
                        .map_err(|_| format!("invalid context size '{s}': must be a number"))?,
                ),
            };
            ProviderConfig::Ollama(OllamaConfig {
                base_url: opt(&v[0]),
                model: opt(&v[1]),
                num_ctx,
            })
        }
        "anthropic" => ProviderConfig::Anthropic(AnthropicConfig {
            api_key: v[0].trim().to_owned(),
            base_url: opt(&v[1]),
            model: opt(&v[2]),
        }),
        "openai" => ProviderConfig::OpenAi(OpenAiConfig {
            api_key: v[0].trim().to_owned(),
            base_url: opt(&v[1]),
            model: opt(&v[2]),
        }),
        other => return Err(format!("unknown provider '{other}'")),
    })
}
