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
    AnthropicConfig, Config, LoginConfig, OllamaConfig, OpenAiConfig, ProviderConfig, ServerConfig,
    SslMode,
};
use crate::theme;
use crate::ui::draw_banner;
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
    ServerLogins(String),
    ServerForm {
        original_name: Option<String>,
    },
    LoginForm {
        server_name: String,
        index: Option<usize>,
    },
    App,
    Provider(&'static str),
    /// Model list of the provider whose form is one level up.
    Models(&'static str),
}

#[derive(Clone)]
enum DeleteConfirmation {
    Server(String),
    Login {
        server_name: String,
        index: usize,
        label: String,
    },
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
    delete_confirmation: Option<DeleteConfirmation>,
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
        match &self.current().node {
            Node::Root => ROOT_ITEMS.iter().map(ToString::to_string).collect(),
            Node::Connections => self
                .config
                .servers()
                .into_iter()
                .map(|server| server.name)
                .collect(),
            Node::ServerLogins(name) => self
                .config
                .servers()
                .into_iter()
                .find(|server| &server.name == name)
                .map(|server| {
                    server
                        .logins
                        .iter()
                        .map(|login| format!("{} / {}", login.login, login.catalog))
                        .collect()
                })
                .unwrap_or_default(),
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
                Node::ServerLogins(name) => name.clone(),
                Node::ServerForm { original_name } => {
                    original_name.as_deref().unwrap_or("New server").to_owned()
                }
                Node::LoginForm { .. } => "Login".to_owned(),
                Node::App => "Application".to_owned(),
                Node::Provider(name) => provider_title(name).to_owned(),
                Node::Models(_) => "Model".to_owned(),
            });
        }
        parts.join(" › ")
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SettingsEvent {
        if let Some(target) = self.delete_confirmation.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.delete_confirmation = None;
                    let result = match target {
                        DeleteConfirmation::Server(name) => self.config.delete_server(&name),
                        DeleteConfirmation::Login {
                            server_name, index, ..
                        } => self.config.delete_login(&server_name, index),
                    };
                    match result {
                        Ok(()) => {
                            let count = self.menu_items().len();
                            self.current_mut().selected =
                                self.current().selected.min(count.saturating_sub(1));
                            self.error = None;
                        }
                        Err(err) => self.error = Some(format!("failed to delete entry: {err}")),
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
                self.add_server();
            }
            KeyCode::Char('-') if matches!(self.current().node, Node::Connections) => {
                if let Some(name) = self.menu_items().get(self.current().selected).cloned() {
                    self.delete_confirmation = Some(DeleteConfirmation::Server(name));
                    self.error = None;
                }
            }
            KeyCode::Char('+') if matches!(self.current().node, Node::ServerLogins(_)) => {
                if let Node::ServerLogins(name) = &self.current().node {
                    self.add_login(name.clone());
                }
            }
            KeyCode::Char('-') if matches!(self.current().node, Node::ServerLogins(_)) => {
                if let Node::ServerLogins(server_name) = &self.current().node
                    && let Some(label) = self.menu_items().get(self.current().selected).cloned()
                {
                    self.delete_confirmation = Some(DeleteConfirmation::Login {
                        server_name: server_name.clone(),
                        index: self.current().selected,
                        label,
                    });
                    self.error = None;
                }
            }
            KeyCode::Char('e') if matches!(self.current().node, Node::Connections) => {
                if let Some(name) = self.menu_items().get(self.current().selected).cloned() {
                    self.edit_server(&name);
                }
            }
            KeyCode::Char('e') if matches!(self.current().node, Node::ServerLogins(_)) => {
                if let Node::ServerLogins(name) = &self.current().node {
                    let name = name.clone();
                    self.edit_server(&name);
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

    fn add_server(&mut self) {
        let existing: Vec<String> = self.menu_items();
        let base_name = "new-server";
        let name = if !existing.iter().any(|name| name == base_name) {
            base_name.to_owned()
        } else {
            (2..)
                .map(|suffix| format!("{base_name}-{suffix}"))
                .find(|candidate| !existing.contains(candidate))
                .expect("an unused server name always exists")
        };
        self.error = None;
        self.path.push(Level::new(
            Node::ServerForm {
                original_name: None,
            },
            Some(server_form(&ServerConfig::with_defaults(name))),
        ));
    }

    fn edit_server(&mut self, name: &str) {
        let Some(server) = self
            .config
            .servers()
            .into_iter()
            .find(|server| server.name == name)
        else {
            self.error = Some(format!("server '{name}' not found"));
            return;
        };
        self.error = None;
        self.path.push(Level::new(
            Node::ServerForm {
                original_name: Some(name.to_owned()),
            },
            Some(server_form(&server)),
        ));
    }

    fn add_login(&mut self, server_name: String) {
        self.error = None;
        self.path.push(Level::new(
            Node::LoginForm {
                server_name,
                index: None,
            },
            Some(login_form(&LoginConfig::with_defaults())),
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
        let node = match self.current().node.clone() {
            Node::Root => match selected {
                0 => Node::Connections,
                1 => Node::App,
                n => Node::Provider(PROVIDERS[n - 2]),
            },
            Node::Connections => match self.menu_items().into_iter().nth(selected) {
                Some(name) => Node::ServerLogins(name),
                None => return,
            },
            Node::ServerLogins(server_name) => {
                if selected >= self.menu_items().len() {
                    return;
                }
                Node::LoginForm {
                    server_name,
                    index: Some(selected),
                }
            }
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
            Node::Root
            | Node::Connections
            | Node::ServerLogins(_)
            | Node::ServerForm { .. }
            | Node::Models(_) => None,
            Node::LoginForm { server_name, index } => {
                let server = self
                    .config
                    .servers()
                    .into_iter()
                    .find(|server| &server.name == server_name)
                    .ok_or_else(|| format!("server '{server_name}' not found"))?;
                let login = if let Some(index) = index {
                    server
                        .logins
                        .get(*index)
                        .cloned()
                        .ok_or_else(|| "login not found".to_owned())?
                } else {
                    LoginConfig::with_defaults()
                };
                Some(login_form(&login))
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
        let node = self.current().node.clone();
        let Some(form) = self.current().form.as_ref() else {
            return Ok(());
        };
        match node {
            Node::ServerForm { original_name } => {
                let old_server = original_name.as_deref().and_then(|name| {
                    self.config
                        .servers()
                        .into_iter()
                        .find(|server| server.name == name)
                });
                let logins = old_server
                    .as_ref()
                    .map(|server| server.logins.clone())
                    .unwrap_or_default();
                let server = server_from_form(form, logins)?;
                if server.name.trim().is_empty() {
                    return Err("server name must not be empty".into());
                }
                server.validate()?;
                self.config
                    .save_server(&server, original_name.as_deref())
                    .map_err(|e| e.to_string())?;
                if let Some(old_name) = original_name.as_deref() {
                    for level in &mut self.path {
                        if let Node::ServerLogins(name) = &mut level.node
                            && name == old_name
                        {
                            *name = server.name.clone();
                        }
                    }
                }
                let parent_index = self.path.len().saturating_sub(2);
                if let Some(parent) = self.path.get_mut(parent_index)
                    && matches!(parent.node, Node::Connections)
                {
                    parent.selected = self
                        .config
                        .servers()
                        .iter()
                        .position(|saved| saved.name == server.name)
                        .unwrap_or(0);
                }
            }
            Node::LoginForm { server_name, index } => {
                let login = login_from_form(form)?;
                login.validate()?;
                self.config
                    .save_login(&server_name, index, &login)
                    .map_err(|e| e.to_string())?;
                let parent_index = self.path.len().saturating_sub(2);
                if let Some(parent) = self.path.get_mut(parent_index)
                    && let Node::ServerLogins(name) = &parent.node
                    && name == &server_name
                {
                    parent.selected = index.unwrap_or_else(|| {
                        self.config
                            .servers()
                            .into_iter()
                            .find(|server| server.name == server_name)
                            .map(|server| server.logins.len().saturating_sub(1))
                            .unwrap_or(0)
                    });
                }
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
            Node::Root | Node::Connections | Node::ServerLogins(_) | Node::Models(_) => {}
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
                    " ↑/↓ select, Enter logins, + server, - delete, e edit, Esc back "
                } else if matches!(level.node, Node::ServerLogins(_)) {
                    " ↑/↓ select, Enter edit login, + login, - delete, e server, Esc back "
                } else {
                    " ↑/↓ to select, Enter to open, Esc to go back "
                },
                Style::default().fg(theme::MUTED),
            ),
        };
        frame.render_widget(Paragraph::new(Line::from(status)), chunks[2]);

        if let Some(target) = &self.delete_confirmation {
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
            let message = match target {
                DeleteConfirmation::Server(name) => format!("Delete server '{name}'?"),
                DeleteConfirmation::Login { label, .. } => format!("Delete login '{label}'?"),
            };
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(format!("{message}\ny/Enter: confirm   n/Esc: cancel"))
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

const SERVER_FIELDS: [&str; 7] = [
    "Server name",
    "IP address / host",
    "Port",
    "SSL mode",
    "Root CA cert (sslrootcert)",
    "Client cert (sslcert)",
    "Client key (sslkey)",
];

const LOGIN_FIELDS: [&str; 3] = ["Login", "Password", "Catalog name"];

fn server_form(server: &ServerConfig) -> Form {
    Form::new(
        "Server",
        &SERVER_FIELDS,
        vec![
            server.name.clone(),
            server.host.clone(),
            server.port.to_string(),
            server.sslmode.to_string(),
            server.sslrootcert.clone().unwrap_or_default(),
            server.sslcert.clone().unwrap_or_default(),
            server.sslkey.clone().unwrap_or_default(),
        ],
    )
    .choice(3, SslMode::ALL.iter().map(ToString::to_string).collect())
}

fn server_from_form(form: &Form, logins: Vec<LoginConfig>) -> Result<ServerConfig, String> {
    let values = &form.values;
    let port = values[2]
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("invalid port '{}': must be a number 1-65535", values[2]))?;
    let optional = |value: &str| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    };
    Ok(ServerConfig {
        name: values[0].trim().to_owned(),
        host: values[1].trim().to_owned(),
        port,
        sslmode: values[3].parse()?,
        sslrootcert: optional(&values[4]),
        sslcert: optional(&values[5]),
        sslkey: optional(&values[6]),
        logins,
    })
}

fn login_form(login: &LoginConfig) -> Form {
    Form::new(
        "Login",
        &LOGIN_FIELDS,
        vec![
            login.login.clone(),
            login.password.clone(),
            login.catalog.clone(),
        ],
    )
    .masked(1)
}

fn login_from_form(form: &Form) -> Result<LoginConfig, String> {
    Ok(LoginConfig {
        login: form.values[0].trim().to_owned(),
        password: form.values[1].clone(),
        catalog: form.values[2].trim().to_owned(),
    })
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
