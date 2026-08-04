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
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
    /// Execute a bare command's real default behavior without reopening its
    /// menu. Used only by the generic command menu.
    RunDefaultCommand(String),
    /// Put text into the main input (palette arg-completion).
    InsertInput(String),
    /// A confirmation dialog approved this.
    Confirmed(ConfirmedAction),
    /// The operator decided about the plan under review. Carries the revision
    /// it was decided about so a decision cannot land on a newer plan.
    ResolvePlan {
        plan_id: String,
        version: u32,
        decision: nexus_agent::PlanDecision,
    },
    /// Refresh/open a view's data.
    Load(LoadRequest),
    AttachSession(String),
    ResumeGoal(String),
    SetTheme(String),
    SubmitGoal(GoalSpec),
    /// Open PERSONA FORGE. `edit` names an existing persona to load; `None`
    /// starts a new one.
    OpenPersonaForge {
        edit: Option<String>,
    },
    /// Save the persona the forge produced, activating it when the operator
    /// chose that on the review step.
    ///
    /// `edit` is the id the forge was opened on, and it is the *only* thing
    /// that decides update-versus-create. Matching by name instead would mean a
    /// new persona that happened to reuse an existing name silently rewrote
    /// that persona rather than reporting the collision.
    SubmitPersona {
        edit: Option<String>,
        spec: Box<nexus_app::persona_service::PersonaSpec>,
    },
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
        model: nexus_models::DiscoveredModel,
        effort: Option<String>,
    },
    PickDiscoveredEffort {
        provider: String,
        base_url: String,
        model: nexus_models::DiscoveredModel,
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
    OpenProvider(String),
    RenameSession {
        title: String,
    },
    CopyText(String),
    ShowHarnessMemory(Box<nexus_core::harness::MemoryRecord>),
    SelectHarnessProfile(String),
    RolloverSummary {
        source_session: String,
        content: String,
    },
    PrepareCommit {
        paths: Vec<String>,
        message: String,
    },
    /// Write typed configuration values in one scope. Carries only the fields
    /// a form actually changed.
    ApplyConfigValues {
        workspace: bool,
        entries: Vec<(String, String)>,
    },
    /// Ask the `/btw` aside sidecar a question from the pop-up. Runs
    /// concurrently with the main turn and never joins the transcript.
    AsideAsk {
        text: String,
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
    /// Choose a session to delete.
    SessionDelete,
    Login,
    Connect,
    Model,
    RefreshModel,
    Agents,
    Plan,
    Tasks,
    Subagents,
    Persona,
    /// Choose a persona to open in the forge.
    PersonaEdit,
    /// Choose a persona to delete.
    PersonaDelete,
    Profile,
    Tools,
    Memory,
    Narrate,
    Activity,
    Rsi,
    Skills,
    Mcp,
    Theme,
    Thinking,
    Details,
    Transcript,
    Help,
    Permissions,
    ReadFormats,
    Sandbox,
    Init,
    Config,
    Budgets,
    Branch,
    Commit,
    Connector,
    CommandMenu(String),
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
    /// Stable identity used for selection/toggle state. Callers should set a
    /// domain id when labels can change.
    pub id: String,
    pub label: String,
    pub category: String,
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
        let label = label.into();
        Self {
            id: label.clone(),
            label,
            category: String::new(),
            badge: String::new(),
            detail: String::new(),
            disabled: None,
            action: Some(action),
        }
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = category.into();
        self
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuFocusRegion {
    Search,
    Items,
    Detail,
    Actions,
}

impl MenuFocusRegion {
    fn next(self) -> Self {
        match self {
            Self::Search => Self::Items,
            Self::Items => Self::Detail,
            Self::Detail => Self::Actions,
            Self::Actions => Self::Search,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Search => Self::Actions,
            Self::Items => Self::Search,
            Self::Detail => Self::Items,
            Self::Actions => Self::Detail,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Items => "items",
            Self::Detail => "detail",
            Self::Actions => "actions",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuSortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSort {
    /// Supported values are `label`, `badge`, `detail`, and `category`.
    pub field: String,
    pub direction: MenuSortDirection,
}

/// Structured menu state shared by every menu route. Rendering consumes this
/// state directly; it never parses report text to infer actions.
#[derive(Debug, Clone)]
pub struct InteractiveMenuState {
    pub menu_id: String,
    pub route: String,
    pub title: String,
    pub brand: Option<BrandVariant>,
    pub items: Vec<MenuItem>,
    pub selected: usize,
    pub selected_item_id: Option<String>,
    pub toggled_item_ids: BTreeSet<String>,
    pub focused_region: MenuFocusRegion,
    pub filter: String,
    pub search_mode: bool,
    pub filters: BTreeMap<String, String>,
    pub sort: Option<MenuSort>,
    pub loading: bool,
    pub error: Option<String>,
    pub empty_message: String,
    pub help_visible: bool,
    pub detail_preview: Option<String>,
    pub parent_route: Option<String>,
    /// Typing filters when true (list menus); false for action menus where
    /// keys like `r` refresh.
    pub searchable: bool,
    /// Extra hint line rendered at the bottom.
    pub hint: String,
    /// Action for Ctrl+R, plus `r` on legacy non-searchable menus.
    pub on_refresh: Option<UiAction>,
    /// Action used by Esc when this is a nested route.
    pub on_back: Option<UiAction>,
}

/// Compatibility name retained for existing menus.
pub type Menu = InteractiveMenuState;

impl InteractiveMenuState {
    pub fn new(title: impl Into<String>, items: Vec<MenuItem>) -> Self {
        let title = title.into();
        let selected_item_id = items.first().map(|item| item.id.clone());
        Self {
            menu_id: title.clone(),
            route: String::new(),
            title,
            brand: None,
            items,
            selected: 0,
            selected_item_id,
            toggled_item_ids: BTreeSet::new(),
            focused_region: MenuFocusRegion::Items,
            filter: String::new(),
            search_mode: false,
            filters: BTreeMap::new(),
            sort: None,
            loading: false,
            error: None,
            empty_message: "nothing matches".into(),
            help_visible: false,
            detail_preview: None,
            parent_route: None,
            searchable: false,
            hint: String::new(),
            on_refresh: None,
            on_back: None,
        }
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.menu_id = id.into();
        self
    }

    pub fn route(mut self, route: impl Into<String>) -> Self {
        self.route = route.into();
        self
    }

    pub fn parent(mut self, route: impl Into<String>, action: UiAction) -> Self {
        self.parent_route = Some(route.into());
        self.on_back = Some(action);
        self
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

    pub fn empty_message(mut self, message: impl Into<String>) -> Self {
        self.empty_message = message.into();
        self
    }

    pub fn sorted(mut self, field: impl Into<String>, direction: MenuSortDirection) -> Self {
        self.sort = Some(MenuSort {
            field: field.into(),
            direction,
        });
        self
    }

    /// Indices of items matching the filter.
    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.to_lowercase();
        let mut visible: Vec<usize> = (0..self.items.len())
            .filter(|&i| {
                let item = &self.items[i];
                (f.is_empty()
                    || item.label.to_lowercase().contains(&f)
                    || item.detail.to_lowercase().contains(&f)
                    || item.badge.to_lowercase().contains(&f)
                    || item.category.to_lowercase().contains(&f))
                    && self.filters.iter().all(|(key, value)| {
                        let expected = value.to_lowercase();
                        expected.is_empty()
                            || match key.as_str() {
                                "category" => item.category.to_lowercase().contains(&expected),
                                "badge" => item.badge.to_lowercase().contains(&expected),
                                "id" => item.id.to_lowercase().contains(&expected),
                                _ => true,
                            }
                    })
            })
            .collect();
        if let Some(sort) = &self.sort {
            visible.sort_by(|left, right| {
                let value = |index: usize| match sort.field.as_str() {
                    "badge" => self.items[index].badge.as_str(),
                    "detail" => self.items[index].detail.as_str(),
                    "category" => self.items[index].category.as_str(),
                    _ => self.items[index].label.as_str(),
                };
                value(*left)
                    .to_ascii_lowercase()
                    .cmp(&value(*right).to_ascii_lowercase())
            });
            if sort.direction == MenuSortDirection::Descending {
                visible.reverse();
            }
        }
        visible
    }

    fn clamp(&mut self) {
        let count = self.visible().len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
        self.selected_item_id = self
            .visible()
            .get(self.selected)
            .map(|index| self.items[*index].id.clone());
    }

    pub fn move_selection(&mut self, delta: isize) {
        if delta < 0 {
            self.selected = self.selected.saturating_sub(delta.unsigned_abs());
        } else {
            self.selected = self.selected.saturating_add(delta as usize);
        }
        self.clamp();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('r')) {
            return self
                .on_refresh
                .clone()
                .map_or(Outcome::Consumed, Outcome::ActionKeepOpen);
        }
        match key.code {
            KeyCode::Esc => {
                if self.help_visible {
                    self.help_visible = false;
                    Outcome::Consumed
                } else if self.search_mode {
                    self.search_mode = false;
                    self.focused_region = MenuFocusRegion::Items;
                    Outcome::Consumed
                } else if self.searchable && !self.filter.is_empty() {
                    self.filter.clear();
                    self.clamp();
                    Outcome::Consumed
                } else if let Some(action) = self.on_back.clone() {
                    Outcome::Action(action)
                } else {
                    Outcome::Close
                }
            }
            KeyCode::Tab => {
                self.focused_region = self.focused_region.next();
                Outcome::Consumed
            }
            KeyCode::BackTab => {
                self.focused_region = self.focused_region.previous();
                Outcome::Consumed
            }
            KeyCode::Char('?') if !self.search_mode => {
                self.help_visible = !self.help_visible;
                Outcome::Consumed
            }
            KeyCode::Char('/') if self.searchable && !self.search_mode => {
                self.search_mode = true;
                self.focused_region = MenuFocusRegion::Search;
                Outcome::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') if !self.search_mode => {
                self.move_selection(-1);
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') if !self.search_mode => {
                self.move_selection(1);
                Outcome::Consumed
            }
            KeyCode::Char('r') if !self.searchable && self.on_refresh.is_some() => {
                Outcome::ActionKeepOpen(self.on_refresh.clone().expect("checked above"))
            }
            KeyCode::Char(' ') if !self.search_mode => {
                let visible = self.visible();
                let Some(&idx) = visible.get(self.selected) else {
                    return Outcome::Consumed;
                };
                let item = &self.items[idx];
                if item.disabled.is_some() {
                    return Outcome::Consumed;
                }
                if !self.toggled_item_ids.remove(&item.id) {
                    self.toggled_item_ids.insert(item.id.clone());
                }
                Outcome::Consumed
            }
            KeyCode::Enter => {
                if self.search_mode || self.focused_region == MenuFocusRegion::Search {
                    self.search_mode = false;
                    self.focused_region = MenuFocusRegion::Items;
                }
                if self.loading || self.error.is_some() {
                    return Outcome::Consumed;
                }
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
            KeyCode::Backspace if self.search_mode => {
                self.filter.pop();
                self.clamp();
                Outcome::Consumed
            }
            KeyCode::Char(c)
                if self.searchable && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.search_mode = true;
                self.focused_region = MenuFocusRegion::Search;
                self.filter.push(c);
                self.selected = 0;
                self.clamp();
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
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

// ---------------------------------------------------------------- /btw aside

/// One question/answer pair in the `/btw` aside. `answer` is `None` while the
/// sidecar is still working.
pub struct AsideExchange {
    pub question: String,
    pub answer: Option<String>,
}

/// The `/btw` aside: a pop-up the operator types into to ask the agent a
/// question or hand it context. Answered by the read-only sidecar concurrently
/// with the main turn — the exchange informs this session as side context but
/// never enters the transcript or disturbs the running inference.
#[derive(Default)]
pub struct AsideChat {
    input: String,
    pub exchanges: Vec<AsideExchange>,
    /// A sidecar call is in flight; block a second until it lands.
    pub pending: bool,
    /// Lines scrolled back from the newest content. Zero follows the bottom,
    /// which is where new questions and answers put it. The renderer clamps
    /// this against the real wrapped height — only it knows the width.
    scroll_back: usize,
}

impl AsideChat {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn scroll_back(&self) -> usize {
        self.scroll_back
    }

    /// Record a new question awaiting an answer and mark a call in flight.
    /// Used both by the Enter key and by `/btw <note>` seeding the first ask.
    pub fn begin(&mut self, question: String) {
        self.exchanges.push(AsideExchange {
            question,
            answer: None,
        });
        self.pending = true;
        self.scroll_back = 0;
    }

    /// Attach the sidecar's reply to the most recent unanswered question.
    pub fn resolve(&mut self, answer: String) {
        if let Some(exchange) = self.exchanges.iter_mut().rev().find(|e| e.answer.is_none()) {
            exchange.answer = Some(answer);
        }
        self.pending = false;
        self.scroll_back = 0;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => Outcome::Close,
            // An answer longer than the pop-up has to be readable from the top.
            KeyCode::Up => {
                self.scroll_back = self.scroll_back.saturating_add(1);
                Outcome::Consumed
            }
            KeyCode::Down => {
                self.scroll_back = self.scroll_back.saturating_sub(1);
                Outcome::Consumed
            }
            KeyCode::PageUp => {
                self.scroll_back = self.scroll_back.saturating_add(10);
                Outcome::Consumed
            }
            KeyCode::PageDown => {
                self.scroll_back = self.scroll_back.saturating_sub(10);
                Outcome::Consumed
            }
            KeyCode::Backspace => {
                self.input.pop();
                Outcome::Consumed
            }
            KeyCode::Enter => {
                let text = self.input.trim().to_string();
                // One aside in flight at a time keeps `resolve` unambiguous.
                if text.is_empty() || self.pending {
                    return Outcome::Consumed;
                }
                self.input.clear();
                self.begin(text.clone());
                self.scroll_back = 0;
                Outcome::ActionKeepOpen(UiAction::AsideAsk { text })
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.push(c);
                Outcome::Consumed
            }
            _ => Outcome::Consumed,
        }
    }
}

// -------------------------------------------------------------- plan review

/// The four answers the operator can give a plan, in the order shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanChoice {
    Approve,
    ApproveWithNote,
    RequestChanges,
    Decline,
}

impl PlanChoice {
    pub const ALL: [PlanChoice; 4] = [
        PlanChoice::Approve,
        PlanChoice::ApproveWithNote,
        PlanChoice::RequestChanges,
        PlanChoice::Decline,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PlanChoice::Approve => "Approve",
            PlanChoice::ApproveWithNote => "Approve with note",
            PlanChoice::RequestChanges => "Request changes",
            PlanChoice::Decline => "Decline",
        }
    }

    /// Whether picking this needs the operator to write something first.
    fn needs_text(self) -> bool {
        matches!(
            self,
            PlanChoice::ApproveWithNote | PlanChoice::RequestChanges
        )
    }

    pub fn prompt(self) -> &'static str {
        match self {
            PlanChoice::ApproveWithNote => "note carried into execution:",
            PlanChoice::RequestChanges => "what should change:",
            _ => "",
        }
    }
}

/// The plan-authorization pop-up.
///
/// A plan review is not a permission check — it is an editorial judgement about
/// proposed work — so it is its own surface with its own answers rather than
/// the generic tool-approval modal, which could only say yes or no to a plan it
/// never showed.
pub struct PlanReview {
    pub request: nexus_agent::PlanReviewRequest,
    pub selected: usize,
    /// Set while the operator is writing a note or a change request.
    pub editor: Option<(PlanChoice, String)>,
    /// Steps scrolled back from the top of the list.
    pub scroll: usize,
    /// A decision has been sent. Guarantees one answer per review however many
    /// times Enter is pressed before the overlay closes.
    pub submitted: bool,
}

impl PlanReview {
    pub fn new(request: nexus_agent::PlanReviewRequest) -> Self {
        Self {
            request,
            selected: 0,
            editor: None,
            scroll: 0,
            submitted: false,
        }
    }

    pub fn choice(&self) -> PlanChoice {
        PlanChoice::ALL[self.selected.min(PlanChoice::ALL.len() - 1)]
    }

    /// Build the action for a choice, or open the editor it needs first.
    fn activate(&mut self, choice: PlanChoice) -> Outcome {
        if self.submitted {
            return Outcome::Consumed;
        }
        if choice.needs_text() {
            self.selected = PlanChoice::ALL
                .iter()
                .position(|c| *c == choice)
                .unwrap_or(0);
            self.editor = Some((choice, String::new()));
            return Outcome::Consumed;
        }
        self.submit(match choice {
            PlanChoice::Decline => nexus_agent::PlanDecision::Decline,
            _ => nexus_agent::PlanDecision::Approve,
        })
    }

    fn submit(&mut self, decision: nexus_agent::PlanDecision) -> Outcome {
        self.submitted = true;
        Outcome::Action(UiAction::ResolvePlan {
            plan_id: self.request.plan_id.clone(),
            version: self.request.version,
            decision,
        })
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        // The editor owns every key while open, so nothing typed into a note
        // reaches the option list or the composer underneath.
        if let Some((choice, text)) = self.editor.as_mut() {
            let choice = *choice;
            return match key.code {
                KeyCode::Esc => {
                    self.editor = None;
                    Outcome::Consumed
                }
                KeyCode::Backspace => {
                    text.pop();
                    Outcome::Consumed
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    text.clear();
                    Outcome::Consumed
                }
                KeyCode::Enter => {
                    let written = text.trim().to_string();
                    // An empty note is not a note; a change request with no
                    // content tells the planner nothing.
                    if written.is_empty() {
                        return Outcome::Consumed;
                    }
                    self.editor = None;
                    self.submit(match choice {
                        PlanChoice::ApproveWithNote => {
                            nexus_agent::PlanDecision::ApproveWithNote(written)
                        }
                        _ => nexus_agent::PlanDecision::RequestChanges(written),
                    })
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    text.push(c);
                    Outcome::Consumed
                }
                _ => Outcome::Consumed,
            };
        }
        match key.code {
            // Dismissing leaves the plan pending rather than deciding it: the
            // pinned panel keeps saying so, and the review reopens from there.
            KeyCode::Esc => Outcome::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                Outcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(PlanChoice::ALL.len() - 1);
                Outcome::Consumed
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(5);
                Outcome::Consumed
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(5);
                Outcome::Consumed
            }
            KeyCode::Enter => {
                let choice = self.choice();
                self.activate(choice)
            }
            KeyCode::Char('a') | KeyCode::Char('A') => self.activate(PlanChoice::Approve),
            KeyCode::Char('n') | KeyCode::Char('N') => self.activate(PlanChoice::ApproveWithNote),
            KeyCode::Char('r') | KeyCode::Char('R') => self.activate(PlanChoice::RequestChanges),
            KeyCode::Char('d') | KeyCode::Char('D') => self.activate(PlanChoice::Decline),
            _ => Outcome::Consumed,
        }
    }
}

// --------------------------------------------------------------------- forms

/// One editable form field.
pub struct FormField {
    pub label: &'static str,
    pub value: String,
    /// The value this field was opened with. A form that writes configuration
    /// compares against it so submitting only persists what was actually
    /// edited, instead of pinning every default as an override.
    pub original: String,
    pub hint: &'static str,
    pub secret: bool,
    /// Heading rendered above this field, starting a new group.
    pub section: Option<&'static str>,
    /// Configuration key this field writes, relative to its block. Empty for
    /// fields that are not a config value (the scope selector).
    pub config_key: &'static str,
}

impl FormField {
    fn new(label: &'static str, value: impl Into<String>, hint: &'static str) -> Self {
        let value = value.into();
        Self {
            label,
            original: value.clone(),
            value,
            hint,
            secret: false,
            section: None,
            config_key: "",
        }
    }

    fn secret(mut self) -> Self {
        self.secret = true;
        self
    }

    fn section(mut self, section: &'static str) -> Self {
        self.section = Some(section);
        self
    }

    fn edited(&self) -> bool {
        self.value.trim() != self.original.trim()
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
    Budgets,
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

    /// Edit the `[limits]` block. Every field is shown with its effective
    /// value so the operator adjusts a number rather than composing a
    /// `/config set` line and guessing the path.
    pub fn budgets(limits: &nexus_core::config::LimitsConfig) -> Self {
        // (label, config key under `limits.`, value, hint)
        let fields = [
            (
                "scope",
                "",
                "workspace".to_string(),
                "workspace = this project · global = all projects",
            ),
            (
                "steps per turn",
                "max_steps_per_turn",
                limits.max_steps_per_turn.to_string(),
                "agent-loop iterations before the turn stops",
            ),
            (
                "model calls per turn",
                "max_model_calls_per_turn",
                limits.max_model_calls_per_turn.to_string(),
                "provider requests in one foreground turn",
            ),
            (
                "tool calls per turn",
                "max_tool_calls_per_turn",
                limits.max_tool_calls_per_turn.to_string(),
                "tool executions in one foreground turn",
            ),
            (
                "retries",
                "max_retries",
                limits.max_retries.to_string(),
                "consecutive invalid model actions before stopping",
            ),
            (
                "repeated calls",
                "max_repeated_calls",
                limits.max_repeated_calls.to_string(),
                "identical tool calls before loop detection stops",
            ),
            (
                "failures per turn",
                "max_failures_per_turn",
                limits.max_failures_per_turn.to_string(),
                "recoverable failures before the turn stops",
            ),
            (
                "tokens per turn",
                "max_tokens_per_turn",
                limits.max_tokens_per_turn.to_string(),
                "input + output ceiling for a metered provider",
            ),
            (
                "self-hosted tokens",
                "self_hosted_max_tokens_per_turn",
                limits.self_hosted_max_tokens_per_turn.to_string(),
                "ceiling for ollama / llamacpp — unmetered tokens",
            ),
            (
                "self-hosted context",
                "self_hosted_context_window",
                limits.self_hosted_context_window.to_string(),
                "window auto-configured per ollama / llamacpp model",
            ),
            (
                "cost per turn (µ)",
                "max_cost_micros_per_turn",
                limits.max_cost_micros_per_turn.to_string(),
                "provider-reported micro-units; 0 disables the check",
            ),
            (
                "completion reserve",
                "completion_reserve_tokens",
                limits.completion_reserve_tokens.to_string(),
                "tokens held back for the answer when packing context",
            ),
            (
                "turn runtime (min)",
                "max_turn_runtime_min",
                limits.max_turn_runtime_min.to_string(),
                "wall-clock ceiling for one foreground turn",
            ),
            (
                "memory writes",
                "max_memory_writes_per_turn",
                limits.max_memory_writes_per_turn.to_string(),
                "durable memory writes one turn may initiate",
            ),
            (
                "subagents per run",
                "max_subagents_per_run",
                limits.max_subagents_per_run.to_string(),
                "subagents created by one root run",
            ),
            (
                "recursion depth",
                "max_recursion_depth",
                limits.max_recursion_depth.to_string(),
                "delegation ancestry depth",
            ),
            (
                "goal steps",
                "goal_step_budget",
                limits.goal_step_budget.to_string(),
                "default step budget for a new goal",
            ),
            (
                "goal runtime (min)",
                "goal_runtime_budget_min",
                limits.goal_runtime_budget_min.to_string(),
                "default wall-clock budget for a new goal",
            ),
        ];
        let sections = [
            ("scope", "where"),
            ("steps per turn", "turn"),
            ("tokens per turn", "tokens & cost"),
            ("turn runtime (min)", "time & delegation"),
            ("goal steps", "goals"),
        ];
        let fields = fields
            .into_iter()
            .map(|(label, key, value, hint)| {
                let mut field = FormField::new(label, value, hint);
                field.config_key = key;
                match sections.iter().find(|(first, _)| *first == label) {
                    Some((_, section)) => field.section(section),
                    None => field,
                }
            })
            .collect();
        Self {
            title: "budgets".into(),
            kind: FormKind::Budgets,
            fields,
            focus: 1,
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
            FormKind::Budgets => {
                let workspace = match self.field("scope").to_ascii_lowercase().as_str() {
                    "workspace" => true,
                    "global" => false,
                    other => {
                        return Err(format!("scope must be workspace or global, got `{other}`"))
                    }
                };
                let mut entries = Vec::new();
                for field in self
                    .fields
                    .iter()
                    .filter(|field| !field.config_key.is_empty())
                {
                    // Every budget is validated, edited or not, so a bad value
                    // elsewhere in the form is reported before anything is
                    // written — but only edits are persisted, so opening the
                    // form and pressing Enter does not pin the defaults.
                    let value = match field.label {
                        // Zero is meaningful: it turns cost enforcement off.
                        "cost per turn (µ)" => parse_non_negative(&field.value, field.label)?,
                        _ => parse_num(&field.value, field.label)?,
                    };
                    if field.edited() {
                        entries.push((format!("limits.{}", field.config_key), value.to_string()));
                    }
                }
                if entries.is_empty() {
                    return Err("no budget was changed".into());
                }
                Ok(UiAction::ApplyConfigValues { workspace, entries })
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

// ----------------------------------------------------------- activity detail

/// One tab of the activity detail overlay. Tabs with no content are never
/// built, so the operator only ever sees tabs that hold something.
pub struct ActivityTab {
    pub title: String,
    pub lines: Vec<String>,
}

/// The Ctrl+E overlay: everything the concise timeline held back for the
/// current turn, grouped by concern and scrollable. Content arrives already
/// sanitized and redacted; this view only arranges it.
pub struct ActivityDetail {
    pub tabs: Vec<ActivityTab>,
    pub selected: usize,
    pub scroll: u16,
    /// Active in-panel search, when the operator has pressed `/`.
    pub search: Option<String>,
    /// True while the search box is accepting keystrokes.
    pub editing_search: bool,
    /// Copy mode renders the active tab unstyled for clean terminal selection.
    pub copy_mode: bool,
}

impl ActivityDetail {
    pub fn new(tabs: Vec<ActivityTab>) -> Self {
        Self {
            tabs,
            selected: 0,
            scroll: 0,
            search: None,
            editing_search: false,
            copy_mode: false,
        }
    }

    pub fn active(&self) -> Option<&ActivityTab> {
        self.tabs.get(self.selected)
    }

    /// Lines of the active tab, filtered by the in-panel search.
    pub fn visible_lines(&self) -> Vec<String> {
        let Some(tab) = self.active() else {
            return Vec::new();
        };
        match self.search.as_deref().filter(|q| !q.is_empty()) {
            Some(query) => {
                let needle = query.to_lowercase();
                tab.lines
                    .iter()
                    .filter(|line| line.to_lowercase().contains(&needle))
                    .cloned()
                    .collect()
            }
            None => tab.lines.clone(),
        }
    }

    fn select(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.selected = index;
            // Scroll is per-tab-view; carrying it across tabs would land the
            // operator in the middle of unrelated content.
            self.scroll = 0;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        if self.editing_search {
            match key.code {
                KeyCode::Esc => {
                    self.editing_search = false;
                    self.search = None;
                    self.scroll = 0;
                }
                KeyCode::Enter => self.editing_search = false,
                KeyCode::Backspace => {
                    if let Some(query) = self.search.as_mut() {
                        query.pop();
                    }
                    self.scroll = 0;
                }
                KeyCode::Char(c) => {
                    self.search.get_or_insert_with(String::new).push(c);
                    self.scroll = 0;
                }
                _ => {}
            }
            return Outcome::Consumed;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Close,
            // Ctrl+E toggles the overlay shut from inside as well as open.
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Outcome::Close
            }
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                let next = (self.selected + 1) % self.tabs.len().max(1);
                self.select(next);
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                let count = self.tabs.len().max(1);
                let prev = (self.selected + count - 1) % count;
                self.select(prev);
            }
            KeyCode::Char(c @ '1'..='9') => {
                self.select(c as usize - '1' as usize);
            }
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            KeyCode::Char('/') => {
                self.editing_search = true;
                self.search = Some(String::new());
            }
            KeyCode::Char('c') => self.copy_mode = !self.copy_mode,
            _ => {}
        }
        Outcome::Consumed
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
#[allow(clippy::large_enum_variant)]
pub enum Overlay {
    Palette(Palette),
    Menu(Box<Menu>),
    Confirm(Confirm),
    Secret(SecretInput),
    Form(Form),
    Pager(Pager),
    Progress(Progress),
    Summary(SummaryPreview),
    ActivityDetail(Box<ActivityDetail>),
    Aside(AsideChat),
    PlanReview(Box<PlanReview>),
    /// The persona editor. Boxed because it carries the full prompt buffer.
    PersonaForge(Box<crate::persona::PersonaForge>),
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
            Overlay::ActivityDetail(d) => d.handle_key(key),
            Overlay::Aside(a) => a.handle_key(key),
            Overlay::PlanReview(p) => p.handle_key(key),
            Overlay::PersonaForge(forge) => match forge.handle_key(key) {
                crate::persona::ForgeOutcome::Consumed => Outcome::Consumed,
                crate::persona::ForgeOutcome::Cancel => Outcome::Close,
                crate::persona::ForgeOutcome::Submit(spec) => {
                    Outcome::Action(UiAction::SubmitPersona {
                        edit: forge.editing.clone(),
                        spec,
                    })
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn modified_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn detail() -> ActivityDetail {
        ActivityDetail::new(vec![
            ActivityTab {
                title: "Activity".into(),
                lines: (0..40).map(|i| format!("activity line {i}")).collect(),
            },
            ActivityTab {
                title: "Tools".into(),
                lines: vec!["fs.read_file · ok".into(), "shell.run · failed".into()],
            },
        ])
    }

    #[test]
    fn activity_detail_switches_tabs_and_resets_scroll() {
        let mut d = detail();
        d.scroll = 12;
        assert!(matches!(d.handle_key(key(KeyCode::Tab)), Outcome::Consumed));
        assert_eq!(d.selected, 1);
        assert_eq!(d.scroll, 0, "a new tab starts at the top");
        d.handle_key(key(KeyCode::Tab));
        assert_eq!(d.selected, 0, "tabs wrap");
        d.handle_key(key(KeyCode::Char('2')));
        assert_eq!(d.selected, 1, "number keys jump directly");
        d.handle_key(key(KeyCode::Char('9')));
        assert_eq!(d.selected, 1, "out-of-range numbers are ignored");
    }

    #[test]
    fn activity_detail_scrolls_and_closes() {
        let mut d = detail();
        d.handle_key(key(KeyCode::PageDown));
        assert_eq!(d.scroll, 10);
        d.handle_key(key(KeyCode::Up));
        assert_eq!(d.scroll, 9);
        d.handle_key(key(KeyCode::Home));
        assert_eq!(d.scroll, 0);
        assert!(matches!(d.handle_key(key(KeyCode::Esc)), Outcome::Close));
        assert!(matches!(
            detail().handle_key(modified_key(KeyCode::Char('e'), KeyModifiers::CONTROL)),
            Outcome::Close,
        ));
    }

    #[test]
    fn activity_detail_search_filters_the_active_tab() {
        let mut d = detail();
        d.selected = 1;
        d.handle_key(key(KeyCode::Char('/')));
        assert!(d.editing_search);
        for c in "failed".chars() {
            d.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(d.visible_lines(), vec!["shell.run · failed".to_string()]);
        // Escape clears the filter rather than closing the overlay.
        assert!(matches!(d.handle_key(key(KeyCode::Esc)), Outcome::Consumed));
        assert!(d.search.is_none());
        assert_eq!(d.visible_lines().len(), 2);
    }

    #[test]
    fn activity_detail_copy_mode_toggles() {
        let mut d = detail();
        assert!(!d.copy_mode);
        d.handle_key(key(KeyCode::Char('c')));
        assert!(d.copy_mode);
        d.handle_key(key(KeyCode::Char('c')));
        assert!(!d.copy_mode);
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
    fn structured_menu_controls_search_focus_toggle_refresh_and_back() {
        let refresh = UiAction::Load(LoadRequest::Model);
        let back = UiAction::RunCommand("profile".into());
        let mut menu = Menu::new(
            "catalog",
            vec![
                MenuItem::new("Alpha", UiAction::RunCommand("alpha".into()))
                    .id("model-alpha")
                    .category("hosted"),
                MenuItem::new("Beta", UiAction::RunCommand("beta".into()))
                    .id("model-beta")
                    .category("local"),
            ],
        )
        .id("model-picker")
        .route("/model")
        .parent("/profile", back.clone())
        .searchable()
        .sorted("label", MenuSortDirection::Descending);
        menu.filters.insert("category".into(), "local".into());
        menu.on_refresh = Some(refresh.clone());

        assert_eq!(menu.visible(), vec![1]);
        assert_eq!(menu.menu_id, "model-picker");
        assert_eq!(menu.route, "/model");

        assert!(matches!(
            menu.handle_key(key(KeyCode::Char('/'))),
            Outcome::Consumed
        ));
        assert!(menu.search_mode);
        assert_eq!(menu.focused_region, MenuFocusRegion::Search);
        menu.handle_key(key(KeyCode::Char('b')));
        assert_eq!(menu.filter, "b");
        menu.handle_key(key(KeyCode::Esc));
        assert!(!menu.search_mode);

        menu.handle_key(key(KeyCode::Char(' ')));
        assert!(menu.toggled_item_ids.contains("model-beta"));
        menu.handle_key(key(KeyCode::Tab));
        assert_eq!(menu.focused_region, MenuFocusRegion::Detail);
        menu.handle_key(key(KeyCode::BackTab));
        assert_eq!(menu.focused_region, MenuFocusRegion::Items);
        menu.handle_key(key(KeyCode::Char('?')));
        assert!(menu.help_visible);
        menu.handle_key(key(KeyCode::Char('?')));
        assert!(!menu.help_visible);

        match menu.handle_key(modified_key(KeyCode::Char('r'), KeyModifiers::CONTROL)) {
            Outcome::ActionKeepOpen(action) => assert_eq!(action, refresh),
            other => panic!("unexpected {:?}", discriminant_name(&other)),
        }

        // Clear the search before the nested back action is eligible.
        menu.handle_key(key(KeyCode::Esc));
        match menu.handle_key(key(KeyCode::Esc)) {
            Outcome::Action(action) => assert_eq!(action, back),
            other => panic!("unexpected {:?}", discriminant_name(&other)),
        }
    }

    #[test]
    fn structured_menu_statuses_disable_activation() {
        let mut loading = Menu::new(
            "loading",
            vec![MenuItem::new(
                "unsafe while loading",
                UiAction::RunCommand("run".into()),
            )],
        )
        .empty_message("no records yet");
        loading.loading = true;
        assert!(matches!(
            loading.handle_key(key(KeyCode::Enter)),
            Outcome::Consumed
        ));
        loading.loading = false;
        loading.error = Some("provider offline".into());
        assert!(matches!(
            loading.handle_key(key(KeyCode::Enter)),
            Outcome::Consumed
        ));
        assert_eq!(loading.empty_message, "no records yet");
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

    fn set_budget(form: &mut Form, label: &str, value: &str) {
        let field = form
            .fields
            .iter_mut()
            .find(|field| field.label == label)
            .unwrap_or_else(|| panic!("no `{label}` field"));
        field.value = value.into();
    }

    #[test]
    fn the_budget_form_writes_only_what_was_edited() {
        let limits = nexus_core::config::LimitsConfig::default();
        let mut form = Form::budgets(&limits);

        // Opening the form and submitting must not pin sixteen defaults as
        // overrides — there is nothing to save.
        assert_eq!(
            form.parse().expect_err("nothing edited"),
            "no budget was changed"
        );

        set_budget(&mut form, "self-hosted tokens", "8000000");
        match form.parse().expect("one edit") {
            UiAction::ApplyConfigValues { workspace, entries } => {
                assert!(workspace, "the form opens on the workspace scope");
                assert_eq!(
                    entries,
                    vec![(
                        "limits.self_hosted_max_tokens_per_turn".to_string(),
                        "8000000".to_string()
                    )],
                );
            }
            other => panic!("expected a config write, got {other:?}"),
        }
    }

    #[test]
    fn the_budget_form_rejects_values_the_config_would_refuse() {
        let limits = nexus_core::config::LimitsConfig::default();
        let mut form = Form::budgets(&limits);

        set_budget(&mut form, "steps per turn", "none");
        assert!(form
            .parse()
            .expect_err("not a number")
            .contains("steps per turn"));

        set_budget(&mut form, "steps per turn", "0");
        assert!(form.parse().expect_err("zero steps").contains("positive"));

        // Zero cost is the documented way to disable cost enforcement, so it
        // must survive the same validation pass.
        set_budget(&mut form, "steps per turn", "24");
        set_budget(&mut form, "cost per turn (µ)", "0");
        set_budget(&mut form, "goal steps", "400");
        match form.parse().expect("valid") {
            UiAction::ApplyConfigValues { entries, .. } => {
                assert_eq!(
                    entries,
                    vec![("limits.goal_step_budget".into(), "400".into())]
                );
            }
            other => panic!("expected a config write, got {other:?}"),
        }

        set_budget(&mut form, "scope", "elsewhere");
        assert!(form
            .parse()
            .expect_err("bad scope")
            .contains("workspace or global"));
    }

    #[test]
    fn the_budget_form_writes_to_the_global_scope_on_request() {
        let limits = nexus_core::config::LimitsConfig::default();
        let mut form = Form::budgets(&limits);
        set_budget(&mut form, "scope", "global");
        set_budget(&mut form, "tokens per turn", "400000");
        match form.parse().expect("valid") {
            UiAction::ApplyConfigValues { workspace, entries } => {
                assert!(!workspace);
                assert_eq!(entries.len(), 1);
            }
            other => panic!("expected a config write, got {other:?}"),
        }
    }

    fn discriminant_name(outcome: &Outcome) -> &'static str {
        match outcome {
            Outcome::Consumed => "Consumed",
            Outcome::Close => "Close",
            Outcome::Action(_) => "Action",
            Outcome::ActionKeepOpen(_) => "ActionKeepOpen",
        }
    }

    #[test]
    fn aside_enter_asks_and_keeps_the_popup_open() {
        let mut aside = AsideChat::new();
        for c in "hi there".chars() {
            assert_eq!(
                discriminant_name(&aside.handle_key(key(KeyCode::Char(c)))),
                "Consumed"
            );
        }
        match aside.handle_key(key(KeyCode::Enter)) {
            Outcome::ActionKeepOpen(UiAction::AsideAsk { text }) => assert_eq!(text, "hi there"),
            other => panic!("expected AsideAsk, got {}", discriminant_name(&other)),
        }
        // The question is recorded, a call is in flight, and the input cleared.
        assert!(aside.pending);
        assert_eq!(aside.exchanges.len(), 1);
        assert!(aside.exchanges[0].answer.is_none());
        assert!(aside.input().is_empty());

        // A second Enter is ignored until the first reply lands.
        assert_eq!(
            discriminant_name(&aside.handle_key(key(KeyCode::Enter))),
            "Consumed"
        );
        aside.handle_key(key(KeyCode::Char('x')));
        assert_eq!(
            discriminant_name(&aside.handle_key(key(KeyCode::Enter))),
            "Consumed"
        );
        assert_eq!(aside.exchanges.len(), 1);

        // The reply attaches to the pending question and frees the next ask.
        aside.resolve("an answer".into());
        assert!(!aside.pending);
        assert_eq!(aside.exchanges[0].answer.as_deref(), Some("an answer"));

        // Esc closes the pop-up.
        assert_eq!(
            discriminant_name(&aside.handle_key(key(KeyCode::Esc))),
            "Close"
        );
    }

    #[test]
    fn aside_scrolls_back_over_a_long_answer_and_follows_new_ones() {
        let mut aside = AsideChat::new();
        aside.begin("what changed?".into());
        aside.resolve("a very long answer".repeat(40));
        assert_eq!(aside.scroll_back(), 0, "a new answer is followed");

        aside.handle_key(key(KeyCode::Up));
        aside.handle_key(key(KeyCode::Up));
        assert_eq!(aside.scroll_back(), 2);
        aside.handle_key(key(KeyCode::PageUp));
        assert_eq!(aside.scroll_back(), 12);
        aside.handle_key(key(KeyCode::Down));
        assert_eq!(aside.scroll_back(), 11);
        aside.handle_key(key(KeyCode::PageDown));
        assert_eq!(aside.scroll_back(), 1);
        // Scrolling past the newest line stops at it rather than going negative.
        aside.handle_key(key(KeyCode::PageDown));
        assert_eq!(aside.scroll_back(), 0);

        // Asking again jumps back to the bottom, where the reply will appear.
        aside.handle_key(key(KeyCode::Up));
        for c in "next".chars() {
            aside.handle_key(key(KeyCode::Char(c)));
        }
        aside.handle_key(key(KeyCode::Enter));
        assert_eq!(aside.scroll_back(), 0);
    }
    fn review_request(version: u32) -> nexus_agent::PlanReviewRequest {
        nexus_agent::PlanReviewRequest {
            plan_id: "plan_1".into(),
            version,
            run_id: "run_1".into(),
            session_id: "sess_1".into(),
            agent: "planner".into(),
            objective: "make the thing work".into(),
            stages: vec![nexus_agent::PlanReviewStage {
                sequence: 1,
                title: "Implement the popup".into(),
                detail: "add the overlay and route its keys".into(),
                files: vec!["crates/nexus-tui/src/views.rs".into()],
            }],
            sandbox_active: true,
        }
    }

    #[test]
    fn plan_review_shortcuts_reach_every_decision() {
        // `a` and `d` answer outright; `n` and `r` need words first.
        let mut review = PlanReview::new(review_request(1));
        match review.handle_key(key(KeyCode::Char('a'))) {
            Outcome::Action(UiAction::ResolvePlan { decision, .. }) => {
                assert_eq!(decision, nexus_agent::PlanDecision::Approve)
            }
            other => panic!("expected approve, got {}", discriminant_name(&other)),
        }

        let mut review = PlanReview::new(review_request(1));
        match review.handle_key(key(KeyCode::Char('d'))) {
            Outcome::Action(UiAction::ResolvePlan { decision, .. }) => {
                assert_eq!(decision, nexus_agent::PlanDecision::Decline)
            }
            other => panic!("expected decline, got {}", discriminant_name(&other)),
        }
    }

    #[test]
    fn plan_review_selection_moves_and_enter_activates_it() {
        let mut review = PlanReview::new(review_request(1));
        assert_eq!(review.choice(), PlanChoice::Approve);
        review.handle_key(key(KeyCode::Down));
        review.handle_key(key(KeyCode::Down));
        review.handle_key(key(KeyCode::Down));
        // Selection stops at the last option rather than wrapping past it.
        review.handle_key(key(KeyCode::Down));
        assert_eq!(review.choice(), PlanChoice::Decline);
        review.handle_key(key(KeyCode::Char('k')));
        assert_eq!(review.choice(), PlanChoice::RequestChanges);
        // Enter on a text option opens the editor instead of deciding.
        assert_eq!(
            discriminant_name(&review.handle_key(key(KeyCode::Enter))),
            "Consumed"
        );
        assert!(review.editor.is_some());
    }

    #[test]
    fn a_note_is_typed_into_the_popup_and_travels_with_the_approval() {
        let mut review = PlanReview::new(review_request(1));
        review.handle_key(key(KeyCode::Char('n')));
        assert!(review.editor.is_some(), "`n` opens the note editor");
        // An empty note submits nothing.
        assert_eq!(
            discriminant_name(&review.handle_key(key(KeyCode::Enter))),
            "Consumed"
        );
        assert!(review.editor.is_some());
        for c in "keep the bindings".chars() {
            assert_eq!(
                discriminant_name(&review.handle_key(key(KeyCode::Char(c)))),
                "Consumed",
                "every keystroke stays in the popup",
            );
        }
        match review.handle_key(key(KeyCode::Enter)) {
            Outcome::Action(UiAction::ResolvePlan { decision, .. }) => assert_eq!(
                decision,
                nexus_agent::PlanDecision::ApproveWithNote("keep the bindings".into())
            ),
            other => panic!(
                "expected an annotated approval, got {}",
                discriminant_name(&other)
            ),
        }
    }

    #[test]
    fn a_change_request_carries_the_operators_words_back() {
        let mut review = PlanReview::new(review_request(2));
        review.handle_key(key(KeyCode::Char('r')));
        for c in "name the files".chars() {
            review.handle_key(key(KeyCode::Char(c)));
        }
        // Esc backs out of the editor without answering.
        review.handle_key(key(KeyCode::Esc));
        assert!(review.editor.is_none());
        assert!(!review.submitted, "backing out is not a decision");

        review.handle_key(key(KeyCode::Char('r')));
        for c in "name the files".chars() {
            review.handle_key(key(KeyCode::Char(c)));
        }
        match review.handle_key(key(KeyCode::Enter)) {
            Outcome::Action(UiAction::ResolvePlan {
                decision, version, ..
            }) => {
                assert_eq!(
                    decision,
                    nexus_agent::PlanDecision::RequestChanges("name the files".into())
                );
                assert_eq!(version, 2, "the answer names the revision it was about");
            }
            other => panic!(
                "expected a change request, got {}",
                discriminant_name(&other)
            ),
        }
    }

    #[test]
    fn a_second_confirmation_cannot_decide_twice() {
        let mut review = PlanReview::new(review_request(1));
        assert!(matches!(
            review.handle_key(key(KeyCode::Enter)),
            Outcome::Action(_)
        ));
        // Repeated Enter must not produce a second action; execution starts once.
        for _ in 0..3 {
            assert_eq!(
                discriminant_name(&review.handle_key(key(KeyCode::Enter))),
                "Consumed",
            );
        }
    }

    #[test]
    fn esc_leaves_the_plan_pending_rather_than_deciding_it() {
        let mut review = PlanReview::new(review_request(1));
        assert_eq!(
            discriminant_name(&review.handle_key(key(KeyCode::Esc))),
            "Close",
            "dismissing closes the popup without answering",
        );
        assert!(!review.submitted);
    }
}
