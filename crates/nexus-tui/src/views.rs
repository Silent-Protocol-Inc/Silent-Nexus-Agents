//! Reusable interactive overlay components: command palette, searchable
//! menus, confirmation dialogs, secure inputs, forms, pagers, and progress
//! dialogs. Components are pure state machines — keys in, [`Outcome`] out —
//! so every flow is unit-testable without a terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nexus_app::providers::CustomEndpointSpec;
use nexus_app::services::GoalSpec;
use nexus_app::{ConfirmedAction, Report};
use nexus_core::brand::BrandVariant;
use nexus_core::SecretString;
use std::collections::VecDeque;

/// What the event loop should do after an overlay consumed a key.
pub enum Outcome {
    /// Nothing further.
    Consumed,
    /// Close this overlay.
    Close,
    /// Close and perform an action.
    Action(UiAction),
    /// Perform an action but keep the overlay open (e.g. refresh).
    ActionKeepOpen(UiAction),
}

/// Actions the overlays can request from the event loop.
#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    /// Execute a slash command line (without the leading `/`).
    RunCommand(String),
    /// Put text into the main input (palette arg-completion).
    InsertInput(String),
    /// A confirmation dialog approved this.
    Confirmed(ConfirmedAction),
    /// Refresh/open a view's data.
    Load(LoadRequest),
    AttachSession(String),
    ResumeGoal(String),
    SetTheme(String),
    SubmitGoal(GoalSpec),
    SubmitCustomEndpoint(CustomEndpointSpec),
    TestCustomEndpoint(CustomEndpointSpec),
    /// Store a provider API key (credential store; `provider/default`).
    StoreProviderKey {
        provider: String,
        key: SecretString,
    },
    /// Codex flows.
    StartDeviceLogin,
    StartClaudeLogin,
    CodexImport,
    CodexApiKey(SecretString),
    /// Cancel the running cancellable operation (device login).
    CancelOp,
    /// Select a configured model entry.
    SelectModel(String),
    /// Persist a discovered local model, then select it.
    UseDiscovered {
        provider: String,
        base_url: String,
        model_id: String,
    },
    /// Open the reasoning-effort picker for a codex plan model (falls through
    /// to the plan default when the model reports no effort choices).
    PickCodexEffort {
        model_id: String,
    },
    /// Persist + select a codex plan model with the chosen effort.
    UseCodexModel {
        model_id: String,
        effort: Option<String>,
    },
    /// Probe one provider and reopen its submenu with fresh data.
    ProbeProvider(String),
    RenameSession {
        title: String,
    },
    CopyText(String),
    RolloverSummary {
        source_session: String,
        content: String,
    },
    PrepareCommit {
        paths: Vec<String>,
        message: String,
    },
}

/// Data loads the event loop performs in the background for views.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadRequest {
    Status,
    Goals,
    GoalMenu,
    GoalDetail(String),
    Resume,
    Sessions,
    Login,
    Model,
    Agents,
    Tasks,
    Subagents,
    Persona,
    Profile,
    Tools,
    Memory,
    Skills,
    Mcp,
    Theme,
    Thinking,
    Details,
    Transcript,
    Help,
    Permissions,
    Sandbox,
    Init,
    Config,
    Branch,
    Commit,
    Connector,
}

// ------------------------------------------------------------------- palette

/// The `/` / Ctrl+K command palette with fuzzy search.
pub struct Palette {
    pub query: String,
    pub selected: usize,
    pub recent: Vec<String>,
}

impl Palette {
    pub fn new(recent: Vec<String>) -> Self {
        Self {
            query: String::new(),
            selected: 0,
            recent,
        }
    }

    /// Split the query into the command word and everything after the first
    /// space, which is treated as arguments (`theme ghost` → `theme` +
    /// `ghost`), so typed argument text never breaks the fuzzy match.
    fn parts(&self) -> (&str, &str) {
        match self.query.split_once(' ') {
            Some((word, rest)) => (word, rest.trim()),
            None => (self.query.as_str(), ""),
        }
    }

    /// Commands matching the command word, recent-first when it is empty.
    pub fn matches(&self) -> Vec<&'static nexus_app::registry::CommandDef> {
        let mut hits = nexus_app::registry::fuzzy(self.parts().0);
        if self.query.is_empty() && !self.recent.is_empty() {
            hits.sort_by_key(|c| {
                self.recent
                    .iter()
                    .position(|r| r == c.name)
                    .unwrap_or(usize::MAX)
            });
        }
        hits
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let count = self.matches().len();
        match key.code {
            KeyCode::Esc => Outcome::Close,
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                Outcome::Consumed
            }
            KeyCode::Down => {
                if count > 0 && self.selected + 1 < count {
                    self.selected += 1;
                }
                Outcome::Consumed
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
                Outcome::Consumed
            }
            KeyCode::Enter => {
                let hits = self.matches();
                let Some(def) = hits.get(self.selected) else {
                    return Outcome::Consumed;
                };
                let args = self.parts().1.to_string();
                if !args.is_empty() {
                    return Outcome::Action(UiAction::RunCommand(format!("{} {args}", def.name)));
                }
                // `[...]` args are optional (bare form opens the interactive
                // view), so Enter runs; only required-arg commands (`<...>`)
                // drop into the input for completion.
                if def.usage.is_empty() || def.usage.starts_with('[') {
                    Outcome::Action(UiAction::RunCommand(def.name.to_string()))
                } else {
                    Outcome::Action(UiAction::InsertInput(format!("/{} ", def.name)))
                }
            }
            KeyCode::Tab => {
                let hits = self.matches();
                if let Some(def) = hits.get(self.selected) {
                    let args = self.parts().1;
                    Outcome::Action(UiAction::InsertInput(format!("/{} {args}", def.name)))
                } else {
                    Outcome::Consumed
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.push(c);
                self.selected = 0;
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }
}

// -------------------------------------------------------------------- menus

/// One selectable row.
#[derive(Debug, Clone)]
pub struct MenuItem {
    pub label: String,
    /// Right-aligned badge (state marker + short status).
    pub badge: String,
    /// Second line of detail under the label.
    pub detail: String,
    /// Why this row cannot be chosen (rendered, selection skips action).
    pub disabled: Option<String>,
    pub action: Option<UiAction>,
}

impl MenuItem {
    pub fn new(label: impl Into<String>, action: UiAction) -> Self {
        Self {
            label: label.into(),
            badge: String::new(),
            detail: String::new(),
            disabled: None,
            action: Some(action),
        }
    }

    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = badge.into();
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn disabled(mut self, reason: impl Into<String>) -> Self {
        self.disabled = Some(reason.into());
        self
    }
}

/// A searchable selection menu (provider picker, goal menu, resume list…).
pub struct Menu {
    pub title: String,
    pub brand: Option<BrandVariant>,
    pub items: Vec<MenuItem>,
    pub selected: usize,
    pub filter: String,
    /// Typing filters when true (list menus); false for action menus where
    /// keys like `r` refresh.
    pub searchable: bool,
    /// Extra hint line rendered at the bottom.
    pub hint: String,
    /// Action for the `r` key (refresh) when not searchable.
    pub on_refresh: Option<UiAction>,
}

impl Menu {
    pub fn new(title: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self {
            title: title.into(),
            brand: None,
            items,
            selected: 0,
            filter: String::new(),
            searchable: false,
            hint: String::new(),
            on_refresh: None,
        }
    }

    pub fn searchable(mut self) -> Self {
        self.searchable = true;
        self
    }

    pub fn branded(mut self, variant: BrandVariant) -> Self {
        self.brand = Some(variant);
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    /// Indices of items matching the filter.
    pub fn visible(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.items.len()).collect();
        }
        let f = self.filter.to_lowercase();
        (0..self.items.len())
            .filter(|&i| {
                self.items[i].label.to_lowercase().contains(&f)
                    || self.items[i].detail.to_lowercase().contains(&f)
            })
            .collect()
    }

    fn clamp(&mut self) {
        let count = self.visible().len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => {
                if self.searchable && !self.filter.is_empty() {
                    self.filter.clear();
                    self.clamp();
                    Outcome::Consumed
                } else {
                    Outcome::Close
                }
            }
            KeyCode::Up | KeyCode::Char('k') if !self.typing(key) => {
                self.selected = self.selected.saturating_sub(1);
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') if !self.typing(key) => {
                self.selected += 1;
                self.clamp();
                Outcome::Consumed
            }
            KeyCode::Char('r') if !self.typing(key) && self.on_refresh.is_some() => {
                Outcome::ActionKeepOpen(self.on_refresh.clone().expect("checked above"))
            }
            KeyCode::Enter => {
                let visible = self.visible();
                let Some(&idx) = visible.get(self.selected) else {
                    return Outcome::Consumed;
                };
                let item = &self.items[idx];
                if item.disabled.is_some() {
                    return Outcome::Consumed;
                }
                match &item.action {
                    Some(action) => Outcome::Action(action.clone()),
                    None => Outcome::Consumed,
                }
            }
            KeyCode::Backspace if self.searchable => {
                self.filter.pop();
                self.clamp();
                Outcome::Consumed
            }
            KeyCode::Char(c)
                if self.searchable && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.filter.push(c);
                self.selected = 0;
                self.clamp();
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }

    /// Whether a char key should type into the filter instead of navigating.
    fn typing(&self, key: KeyEvent) -> bool {
        self.searchable && matches!(key.code, KeyCode::Char(_))
    }
}

// -------------------------------------------------------------- confirmation

/// Explicit yes/no dialog for destructive actions.
pub struct Confirm {
    pub title: String,
    pub body: Vec<String>,
    pub action: UiAction,
    /// Extra emphasis line, e.g. what will NOT be touched.
    pub note: Option<String>,
    pub scroll: usize,
}

impl Confirm {
    pub fn for_action(action: ConfirmedAction) -> Self {
        let note = match &action {
            ConfirmedAction::LogoutCodex => {
                Some("Your own `codex` CLI login is NOT touched.".to_string())
            }
            ConfirmedAction::UseExistingCodex => {
                Some("Consent can be revoked at any time with /auth revoke-existing.".to_string())
            }
            ConfirmedAction::UseExistingClaude => Some(
                "Consent can be revoked with /auth revoke-existing-claude; Claude tools stay disabled."
                    .to_string(),
            ),
            _ => None,
        };
        Self {
            title: "confirm".into(),
            body: action.prompt().lines().map(String::from).collect(),
            action: UiAction::Confirmed(action),
            note,
            scroll: 0,
        }
    }

    pub fn custom(title: impl Into<String>, body: Vec<String>, action: UiAction) -> Self {
        Self {
            title: title.into(),
            body,
            action,
            note: None,
            scroll: 0,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Outcome::Action(self.action.clone()),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Enter => {
                Outcome::Close
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = (self.scroll + 1).min(self.body.len().saturating_sub(1));
                Outcome::Consumed
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(8);
                Outcome::Consumed
            }
            KeyCode::PageDown => {
                self.scroll = (self.scroll + 8).min(self.body.len().saturating_sub(1));
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }
}

// ------------------------------------------------------------- secret input

/// Masked input for API keys/tokens. The value never lands in history or
/// logs; on submit it is wrapped in [`SecretString`] immediately.
pub struct SecretInput {
    pub title: String,
    pub prompt: String,
    value: String,
    /// Builds the action from the captured secret.
    pub target: SecretTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SecretTarget {
    CodexApiKey,
    Provider(String),
}

impl SecretInput {
    pub fn new(title: impl Into<String>, prompt: impl Into<String>, target: SecretTarget) -> Self {
        Self {
            title: title.into(),
            prompt: prompt.into(),
            value: String::new(),
            target,
        }
    }

    pub fn masked(&self) -> String {
        "•".repeat(self.value.chars().count())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => Outcome::Close,
            KeyCode::Backspace => {
                self.value.pop();
                Outcome::Consumed
            }
            KeyCode::Enter => {
                if self.value.trim().is_empty() {
                    return Outcome::Consumed;
                }
                let secret = SecretString::new(std::mem::take(&mut self.value));
                Outcome::Action(match &self.target {
                    SecretTarget::CodexApiKey => UiAction::CodexApiKey(secret),
                    SecretTarget::Provider(p) => UiAction::StoreProviderKey {
                        provider: p.clone(),
                        key: secret,
                    },
                })
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.value.push(c);
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }
}

// --------------------------------------------------------------------- forms

/// One editable form field.
pub struct FormField {
    pub label: &'static str,
    pub value: String,
    pub hint: &'static str,
    pub secret: bool,
}

impl FormField {
    fn new(label: &'static str, value: impl Into<String>, hint: &'static str) -> Self {
        Self {
            label,
            value: value.into(),
            hint,
            secret: false,
        }
    }

    fn secret(mut self) -> Self {
        self.secret = true;
        self
    }

    pub fn shown_value(&self) -> String {
        if self.secret {
            "•".repeat(self.value.chars().count())
        } else {
            self.value.clone()
        }
    }
}

/// Which flow a form belongs to; drives parsing on submit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FormKind {
    GoalCreate,
    CustomEndpoint,
    SessionTitle,
    GitCommit,
}

/// A guided multi-field form (goal creation, custom endpoints).
pub struct Form {
    pub title: String,
    pub kind: FormKind,
    pub fields: Vec<FormField>,
    pub focus: usize,
    pub error: Option<String>,
}

impl Form {
    pub fn goal_create(default_steps: i64, default_minutes: i64) -> Self {
        Self {
            title: "create goal".into(),
            kind: FormKind::GoalCreate,
            fields: vec![
                FormField::new("objective", "", "what must be accomplished (required)"),
                FormField::new("title", "", "short name (defaults to the objective)"),
                FormField::new(
                    "criteria",
                    "",
                    "acceptance criteria, `;`-separated — empty = never verifiable",
                ),
                FormField::new("constraints", "", "hard constraints, `;`-separated"),
                FormField::new(
                    "allowed paths",
                    "",
                    "restrict writes to these globs, `;`-separated",
                ),
                FormField::new(
                    "prohibited paths",
                    "",
                    "never touch these globs, `;`-separated",
                ),
                FormField::new("step budget", default_steps.to_string(), "max agent steps"),
                FormField::new(
                    "token budget",
                    "0",
                    "total input + output tokens; 0 = unlimited",
                ),
                FormField::new(
                    "runtime budget (min)",
                    default_minutes.to_string(),
                    "max wall-clock minutes",
                ),
            ],
            focus: 0,
            error: None,
        }
    }

    pub fn custom_endpoint() -> Self {
        Self {
            title: "custom endpoint".into(),
            kind: FormKind::CustomEndpoint,
            fields: vec![
                FormField::new("profile name", "", "config entry name, e.g. my_gateway"),
                FormField::new(
                    "protocol",
                    "openai_compatible",
                    "openai_compatible | ollama | llamacpp",
                ),
                FormField::new(
                    "endpoint",
                    "",
                    "host:port or full URL; /v1 is added for OpenAI presets",
                ),
                FormField::new("use tls", "yes", "yes = https for host-only values"),
                FormField::new(
                    "verify tls",
                    "yes",
                    "disable only for a specifically trusted self-signed endpoint",
                ),
                FormField::new(
                    "api key",
                    "",
                    "stored in the credential store; empty = none",
                )
                .secret(),
                FormField::new("model id", "", "model identifier the server expects"),
                FormField::new("context window", "8192", "prompt+completion token limit"),
                FormField::new("max output tokens", "2048", "completion token limit"),
                FormField::new("timeout (s)", "120", "request timeout"),
            ],
            focus: 0,
            error: None,
        }
    }

    pub fn session_title(current: &str) -> Self {
        Self {
            title: "session title".into(),
            kind: FormKind::SessionTitle,
            fields: vec![FormField::new(
                "title",
                current,
                "persisted on the active session",
            )],
            focus: 0,
            error: None,
        }
    }

    pub fn git_commit(paths: &[String]) -> Self {
        Self {
            title: "local git commit".into(),
            kind: FormKind::GitCommit,
            fields: vec![
                FormField::new(
                    "files",
                    paths.join(";"),
                    "selected workspace paths, `;`-separated (required)",
                ),
                FormField::new("message", "", "commit message (required)"),
            ],
            focus: 0,
            error: None,
        }
    }

    fn field(&self, label: &str) -> String {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map(|f| f.value.trim().to_string())
            .unwrap_or_default()
    }

    fn list_field(&self, label: &str) -> Vec<String> {
        self.field(label)
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    }

    /// Parse into the target spec; `Err` sets an inline form error.
    pub fn parse(&self) -> Result<UiAction, String> {
        match self.kind {
            FormKind::GoalCreate => {
                let objective = self.field("objective");
                if objective.is_empty() {
                    return Err("objective is required".into());
                }
                let title = {
                    let t = self.field("title");
                    if t.is_empty() {
                        objective.chars().take(80).collect()
                    } else {
                        t
                    }
                };
                let step_budget = parse_num(&self.field("step budget"), "step budget")?;
                let token_budget = parse_non_negative(&self.field("token budget"), "token budget")?;
                let runtime = parse_num(&self.field("runtime budget (min)"), "runtime budget")?;
                Ok(UiAction::SubmitGoal(GoalSpec {
                    title,
                    objective,
                    acceptance_criteria: self.list_field("criteria"),
                    constraints: self.list_field("constraints"),
                    allowed_paths: self.list_field("allowed paths"),
                    prohibited_paths: self.list_field("prohibited paths"),
                    step_budget: Some(step_budget),
                    token_budget: Some(token_budget),
                    runtime_budget_min: Some(runtime),
                }))
            }
            FormKind::CustomEndpoint => {
                let spec = self.custom_endpoint_spec()?;
                nexus_app::providers::validate_custom_endpoint(&spec)
                    .map_err(|error| error.to_string())?;
                Ok(UiAction::SubmitCustomEndpoint(spec))
            }
            FormKind::SessionTitle => {
                let title = self.field("title");
                if title.is_empty() {
                    return Err("title cannot be empty".into());
                }
                Ok(UiAction::RenameSession { title })
            }
            FormKind::GitCommit => {
                let paths = self.list_field("files");
                let message = self.field("message");
                if paths.is_empty() || message.is_empty() {
                    return Err("selected files and a commit message are required".into());
                }
                Ok(UiAction::PrepareCommit { paths, message })
            }
        }
    }

    /// Build the endpoint spec (shared by save and test-connection).
    pub fn custom_endpoint_spec(&self) -> Result<CustomEndpointSpec, String> {
        let key = self.field("api key");
        let spec = CustomEndpointSpec {
            name: self.field("profile name"),
            protocol: self.field("protocol"),
            base_url: self.field("endpoint"),
            use_tls: parse_bool(&self.field("use tls"), "use tls")?,
            tls_verify: parse_bool(&self.field("verify tls"), "verify tls")?,
            api_key: if key.is_empty() {
                None
            } else {
                Some(SecretString::new(key))
            },
            model: self.field("model id"),
            context_window: parse_num(&self.field("context window"), "context window")? as usize,
            max_output_tokens: parse_num(&self.field("max output tokens"), "max output tokens")?
                as usize,
            timeout_secs: parse_num(&self.field("timeout (s)"), "timeout")? as u64,
        };
        nexus_app::providers::normalize_custom_endpoint_url(&spec).map_err(|e| e.to_string())?;
        Ok(spec)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => Outcome::Close,
            KeyCode::Up | KeyCode::BackTab => {
                self.focus = self.focus.saturating_sub(1);
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Tab => {
                if self.focus + 1 < self.fields.len() {
                    self.focus += 1;
                }
                Outcome::Consumed
            }
            KeyCode::Backspace => {
                self.fields[self.focus].value.pop();
                self.error = None;
                Outcome::Consumed
            }
            // Ctrl+T: test connection (custom endpoint only).
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.kind == FormKind::CustomEndpoint {
                    match self.custom_endpoint_spec() {
                        Ok(spec) => Outcome::ActionKeepOpen(UiAction::TestCustomEndpoint(spec)),
                        Err(e) => {
                            self.error = Some(e);
                            Outcome::Consumed
                        }
                    }
                } else {
                    Outcome::Consumed
                }
            }
            KeyCode::Enter => {
                if self.focus + 1 < self.fields.len() {
                    self.focus += 1;
                    return Outcome::Consumed;
                }
                match self.parse() {
                    Ok(action) => Outcome::Action(action),
                    Err(e) => {
                        self.error = Some(e);
                        Outcome::Consumed
                    }
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.fields[self.focus].value.push(c);
                self.error = None;
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }
}

fn parse_num(text: &str, label: &str) -> Result<i64, String> {
    text.parse::<i64>()
        .map_err(|_| format!("{label} must be a number, got `{text}`"))
        .and_then(|n| {
            if n > 0 {
                Ok(n)
            } else {
                Err(format!("{label} must be positive"))
            }
        })
}

fn parse_non_negative(text: &str, label: &str) -> Result<i64, String> {
    text.parse::<i64>()
        .map_err(|_| format!("{label} must be a number, got `{text}`"))
        .and_then(|n| {
            if n >= 0 {
                Ok(n)
            } else {
                Err(format!("{label} must be zero or positive"))
            }
        })
}

fn parse_bool(text: &str, label: &str) -> Result<bool, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" | "1" => Ok(true),
        "no" | "false" | "off" | "0" => Ok(false),
        _ => Err(format!("{label} must be yes or no")),
    }
}

// --------------------------------------------------------------------- pager

/// Scrollable report view (status panel, help, diff, config…).
pub struct Pager {
    pub title: String,
    pub report: Report,
    pub scroll: u16,
    /// `r` re-runs this load.
    pub refresh: Option<LoadRequest>,
}

impl Pager {
    pub fn new(title: impl Into<String>, report: Report) -> Self {
        Self {
            title: title.into(),
            report,
            scroll: 0,
            refresh: None,
        }
    }

    pub fn refreshable(mut self, load: LoadRequest) -> Self {
        self.refresh = Some(load);
        self
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Outcome::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                Outcome::Consumed
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                Outcome::Consumed
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10);
                Outcome::Consumed
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.scroll = 0;
                Outcome::Consumed
            }
            KeyCode::Char('r') => match &self.refresh {
                Some(load) => Outcome::ActionKeepOpen(UiAction::Load(load.clone())),
                None => Outcome::Consumed,
            },
            _ => Outcome::Consumed,
        }
    }
}

// ------------------------------------------------------------------ progress

/// Live progress dialog (device login): streams lines, shows the
/// verification URL and code prominently, cancellable while running.
pub struct Progress {
    pub title: String,
    pub lines: VecDeque<String>,
    pub url: Option<String>,
    pub code: Option<String>,
    pub done: bool,
    pub failed: bool,
}

// --------------------------------------------------------------- summary

pub struct SummaryPreview {
    pub session_id: String,
    pub content: String,
    pub path: String,
    pub clipboard_status: String,
    pub scroll: u16,
}

impl SummaryPreview {
    pub fn new(
        session_id: String,
        content: String,
        path: String,
        clipboard_status: String,
    ) -> Self {
        Self {
            session_id,
            content,
            path,
            clipboard_status,
            scroll: 0,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Outcome::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                Outcome::Consumed
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                Outcome::Consumed
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(10);
                Outcome::Consumed
            }
            KeyCode::Char('c') => Outcome::ActionKeepOpen(UiAction::CopyText(self.content.clone())),
            KeyCode::Char('r') | KeyCode::Char('y') | KeyCode::Enter => {
                Outcome::Action(UiAction::RolloverSummary {
                    source_session: self.session_id.clone(),
                    content: self.content.clone(),
                })
            }
            _ => Outcome::Consumed,
        }
    }
}

impl Progress {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            lines: VecDeque::new(),
            url: None,
            code: None,
            done: false,
            failed: false,
        }
    }

    pub fn push_line(&mut self, line: String) {
        self.lines.push_back(line);
        while self.lines.len() > 8 {
            self.lines.pop_front();
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => {
                if self.done {
                    Outcome::Close
                } else {
                    Outcome::Action(UiAction::CancelOp)
                }
            }
            KeyCode::Enter | KeyCode::Char('q') if self.done => Outcome::Close,
            _ => Outcome::Consumed,
        }
    }
}

// ------------------------------------------------------------------ overlays

/// The overlay stack element.
pub enum Overlay {
    Palette(Palette),
    Menu(Menu),
    Confirm(Confirm),
    Secret(SecretInput),
    Form(Form),
    Pager(Pager),
    Progress(Progress),
    Summary(SummaryPreview),
}

impl Overlay {
    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match self {
            Overlay::Palette(p) => p.handle_key(key),
            Overlay::Menu(m) => m.handle_key(key),
            Overlay::Confirm(c) => c.handle_key(key),
            Overlay::Secret(s) => s.handle_key(key),
            Overlay::Form(f) => f.handle_key(key),
            Overlay::Pager(p) => p.handle_key(key),
            Overlay::Progress(p) => p.handle_key(key),
            Overlay::Summary(s) => s.handle_key(key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn palette_fuzzy_and_enter() {
        let mut p = Palette::new(vec![]);
        for c in "go".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        let names: Vec<&str> = p.matches().iter().map(|c| c.name).take(3).collect();
        assert!(
            names.contains(&"goal") || names.contains(&"goals"),
            "{names:?}"
        );
        // /goal's args are optional (bare form opens the menu) → Enter runs.
        let idx = p
            .matches()
            .iter()
            .position(|c| c.name == "goal")
            .expect("goal");
        p.selected = idx;
        match p.handle_key(key(KeyCode::Enter)) {
            Outcome::Action(UiAction::RunCommand(text)) => assert_eq!(text, "goal"),
            other => panic!("unexpected {:?}", discriminant_name(&other)),
        }
        // Required-arg commands still drop into the input for completion.
        let mut p = Palette::new(vec![]);
        let idx = p
            .matches()
            .iter()
            .position(|c| c.usage.starts_with('<'))
            .expect("a required-arg command exists");
        let name = p.matches()[idx].name;
        p.selected = idx;
        match p.handle_key(key(KeyCode::Enter)) {
            Outcome::Action(UiAction::InsertInput(text)) => assert_eq!(text, format!("/{name} ")),
            other => panic!("unexpected {:?}", discriminant_name(&other)),
        }
    }

    #[test]
    fn palette_query_args_run_with_the_command() {
        let mut p = Palette::new(vec![]);
        for c in "theme ghost".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        let idx = p
            .matches()
            .iter()
            .position(|c| c.name == "theme")
            .expect("theme matches despite trailing args");
        p.selected = idx;
        match p.handle_key(key(KeyCode::Enter)) {
            Outcome::Action(UiAction::RunCommand(line)) => assert_eq!(line, "theme ghost"),
            other => panic!("unexpected {:?}", discriminant_name(&other)),
        }
    }

    #[test]
    fn palette_runs_argless_commands() {
        let mut p = Palette::new(vec![]);
        for c in "about".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        match p.handle_key(key(KeyCode::Enter)) {
            Outcome::Action(UiAction::RunCommand(name)) => assert_eq!(name, "about"),
            other => panic!("unexpected {:?}", discriminant_name(&other)),
        }
    }

    #[test]
    fn menu_navigation_filter_and_disabled() {
        let mut m = Menu::new(
            "test",
            vec![
                MenuItem::new("Alpha", UiAction::RunCommand("a".into())),
                MenuItem::new("Beta", UiAction::RunCommand("b".into())).disabled("not ready"),
                MenuItem::new("Gamma", UiAction::RunCommand("g".into())),
            ],
        )
        .searchable();
        // Filter to Gamma.
        for c in "gam".chars() {
            m.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(m.visible(), vec![2]);
        match m.handle_key(key(KeyCode::Enter)) {
            Outcome::Action(UiAction::RunCommand(cmd)) => assert_eq!(cmd, "g"),
            other => panic!("unexpected {:?}", discriminant_name(&other)),
        }
        // Disabled rows don't fire.
        m.filter.clear();
        m.selected = 1;
        assert!(matches!(
            m.handle_key(key(KeyCode::Enter)),
            Outcome::Consumed
        ));
    }

    #[test]
    fn confirm_defaults_to_no() {
        let mut c = Confirm::for_action(ConfirmedAction::RevertFile("a.txt".into()));
        assert!(matches!(c.handle_key(key(KeyCode::Enter)), Outcome::Close));
        assert!(matches!(c.handle_key(key(KeyCode::Esc)), Outcome::Close));
        match c.handle_key(key(KeyCode::Char('y'))) {
            Outcome::Action(UiAction::Confirmed(ConfirmedAction::RevertFile(p))) => {
                assert_eq!(p, "a.txt")
            }
            other => panic!("unexpected {:?}", discriminant_name(&other)),
        }
    }

    #[test]
    fn confirm_body_can_scroll_without_changing_safe_default() {
        let mut confirm = Confirm::custom(
            "review",
            (0..30).map(|index| format!("line {index}")).collect(),
            UiAction::RunCommand("commit".into()),
        );
        assert!(matches!(
            confirm.handle_key(key(KeyCode::PageDown)),
            Outcome::Consumed
        ));
        assert_eq!(confirm.scroll, 8);
        assert!(matches!(
            confirm.handle_key(key(KeyCode::Enter)),
            Outcome::Close
        ));
    }

    #[test]
    fn secret_input_masks_and_wraps() {
        let mut s = SecretInput::new(
            "api key",
            "OpenAI key",
            SecretTarget::Provider("openai".into()),
        );
        for c in "sk-abc".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(s.masked(), "••••••");
        match s.handle_key(key(KeyCode::Enter)) {
            Outcome::Action(UiAction::StoreProviderKey { provider, key }) => {
                assert_eq!(provider, "openai");
                assert_eq!(key.expose(), "sk-abc");
                assert_eq!(format!("{key:?}"), "[redacted]");
            }
            other => panic!("unexpected {:?}", discriminant_name(&other)),
        }
    }

    #[test]
    fn goal_form_requires_objective_then_submits() {
        let mut f = Form::goal_create(50, 60);
        // Submit from the last field with no objective → error.
        f.focus = f.fields.len() - 1;
        assert!(matches!(
            f.handle_key(key(KeyCode::Enter)),
            Outcome::Consumed
        ));
        assert!(f.error.as_deref().unwrap_or_default().contains("objective"));
        // Fill the objective and criteria, then submit.
        f.fields[0].value = "Fix the parser".into();
        f.fields[2].value = "tests pass; docs updated".into();
        f.focus = f.fields.len() - 1;
        match f.handle_key(key(KeyCode::Enter)) {
            Outcome::Action(UiAction::SubmitGoal(spec)) => {
                assert_eq!(spec.objective, "Fix the parser");
                assert_eq!(spec.acceptance_criteria.len(), 2);
                assert_eq!(spec.step_budget, Some(50));
            }
            other => panic!("unexpected {:?}", discriminant_name(&other)),
        }
    }

    #[test]
    fn custom_endpoint_form_validates_url() {
        let mut f = Form::custom_endpoint();
        f.fields[0].value = "gw".into();
        f.fields[2].value = "not a url".into();
        f.fields[6].value = "m".into();
        f.focus = f.fields.len() - 1;
        assert!(matches!(
            f.handle_key(key(KeyCode::Enter)),
            Outcome::Consumed
        ));
        assert!(f.error.is_some());
        f.fields[2].value = "gw.example".into();
        f.error = None;
        match f.handle_key(key(KeyCode::Enter)) {
            Outcome::Action(UiAction::SubmitCustomEndpoint(spec)) => {
                assert_eq!(spec.name, "gw");
                assert!(spec.api_key.is_none());
                assert!(spec.use_tls);
                assert!(spec.tls_verify);
            }
            other => panic!("unexpected {:?}", discriminant_name(&other)),
        }
    }

    #[test]
    fn progress_cancel_vs_close() {
        let mut p = Progress::new("device login");
        assert!(matches!(
            p.handle_key(key(KeyCode::Esc)),
            Outcome::Action(UiAction::CancelOp)
        ));
        p.done = true;
        assert!(matches!(p.handle_key(key(KeyCode::Esc)), Outcome::Close));
    }

    fn discriminant_name(outcome: &Outcome) -> &'static str {
        match outcome {
            Outcome::Consumed => "Consumed",
            Outcome::Close => "Close",
            Outcome::Action(_) => "Action",
            Outcome::ActionKeepOpen(_) => "ActionKeepOpen",
        }
    }
}
