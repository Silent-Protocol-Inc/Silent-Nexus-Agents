//! The boot snapshot: what NEXUS knows about itself when it comes up.
//!
//! Startup used to be a brand reveal followed by three unrelated `st.system(…)`
//! lines pushed from two different files. Replacing those with a wake flow made
//! the *content* coherent but kept the delivery: each line still went through
//! the ordinary timeline renderer, which classified it as a completed `Notice`
//! and drew a `✓ DONE  NOTICE` header over it. Four startup facts therefore
//! opened every session as four completed tasks that nobody performed.
//!
//! So boot state is no longer an event at all. This module gathers the facts
//! once, as data; the TUI renders them as one welcome panel and the CLI prints
//! them as a compact block. Neither writes to the timeline.
//!
//! Two rules govern the whole snapshot:
//!
//! * **A field with nothing real to say is `None`, never a placeholder.** A
//!   fresh workspace has no session to restore and no memory to link, so those
//!   sections do not exist rather than reading "Session: none".
//! * **Nothing here is an outcome.** Startup is not a task that succeeded, so
//!   nothing is marked done, checked off, or counted as work.

use crate::app::App;

/// Everything the welcome panel can show, gathered once at startup.
///
/// Deliberately plain data: no styling, no widths, no glyphs. The renderer
/// decides how much of it fits, which is what lets the same snapshot drive a
/// full-width panel, a mobile-portrait stack, and a non-TTY text block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BootSnapshot {
    /// Running version, for the identity row.
    pub version: String,
    /// Workspace, already shortened for display (`~/project`).
    pub workspace: String,
    /// Provider and model, as the status bar words it.
    pub model: String,
    /// Active agent role.
    pub agent: String,
    /// Sandbox isolation level, and the permission mode when it is not the
    /// default — the two together are what "what can this thing do" means.
    pub access: String,
    /// The line under the wordmark. Never a name the profile system has not
    /// actually given us.
    pub greeting: String,
    /// Previous session, when there was one.
    pub session: Option<SessionState>,
    /// Memory, when there is anything stored or waiting.
    pub memory: Option<MemoryState>,
    /// Exactly one headline, shown once per version.
    pub update: Option<String>,
    /// Two or three commands that are actually useful right now.
    pub tips: Vec<Tip>,
    /// Subsystems that came up degraded. Almost always empty.
    pub notices: Vec<BootNotice>,
}

/// What the operator was last doing here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    /// Session title, when it has one worth showing.
    pub title: Option<String>,
    /// Git branch at restore time.
    pub branch: Option<String>,
    /// Concise local date — never the raw RFC-3339 stamp, which is a machine's
    /// way of saying "yesterday afternoon".
    pub when: Option<String>,
    /// Whether there is work `/resume` would pick up.
    pub resumable: bool,
}

/// What the harness remembers about this workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryState {
    /// Facts in use.
    pub linked: usize,
    /// Facts waiting on a human, when recording is approval-gated.
    pub pending: usize,
    /// Self-improvement candidates waiting on a human.
    pub improvements: usize,
}

/// One command worth knowing about right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tip {
    pub command: String,
    pub detail: String,
}

/// How loudly a degraded subsystem should read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    /// Reduced capability; the session continues.
    Degraded,
    /// Something the operator has to act on before the agent is useful.
    Blocked,
}

/// A subsystem that did not come up cleanly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootNotice {
    /// `MEMORY`, `SESSION`, `MODELS` — the area, not a module path.
    pub subsystem: String,
    pub level: NoticeLevel,
    pub detail: String,
}

impl BootSnapshot {
    /// Gather the snapshot.
    ///
    /// `model` is passed in rather than derived here because provider labelling
    /// (`Ollama`, `Codex`, auth state) belongs to the surface that already owns
    /// the status bar; duplicating it would give the panel and the bar two
    /// different ways to name the same model.
    pub fn gather(app: &App, model: impl Into<String>) -> Self {
        let memory = memory_state(app);
        let session = session_state(app);
        let notices = notices(app);
        let tips = tips(app, session.as_ref(), memory.as_ref());
        Self {
            version: nexus_core::brand::VERSION.to_string(),
            workspace: display_path(&app.workspace_key),
            model: model.into(),
            agent: app.active_agent(),
            access: access_line(app),
            greeting: greeting(app, session.is_some()),
            session,
            memory,
            update: whats_new(app),
            tips,
            notices,
        }
    }

    /// The collapsed form, for after the first turn: identity and the four
    /// facts that stay true all session.
    pub fn compact(&self) -> String {
        let mut parts = vec![
            nexus_core::brand::PRODUCT.to_string(),
            self.agent.clone(),
            self.model.clone(),
        ];
        if let Some(branch) = self.session.as_ref().and_then(|s| s.branch.clone()) {
            parts.push(branch);
        }
        if self.session.is_some() {
            parts.push("restored".into());
        }
        parts.join(" · ")
    }

    /// Plain-text lines for a surface with no panel to draw — `--inline` under a
    /// pipe, a non-TTY run, a log. No borders, no color, no icons.
    pub fn plain_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("{} {}", nexus_core::brand::PRODUCT, self.version)];
        lines.push(format!("Workspace: {}", self.workspace));
        lines.push(format!("Model: {}", self.model));
        lines.push(format!("Agent: {}", self.agent));
        lines.push(format!("Access: {}", self.access));
        if let Some(session) = &self.session {
            lines.push(format!("Session: {}", session.summary()));
        }
        if let Some(memory) = &self.memory {
            if let Some(summary) = memory.summary() {
                lines.push(format!("Memory: {summary}"));
            }
        }
        if let Some(update) = &self.update {
            lines.push(format!("Update: {update}"));
        }
        for notice in &self.notices {
            lines.push(format!("{}: {}", notice.subsystem, notice.detail));
        }
        for tip in &self.tips {
            lines.push(format!("Tip: {} {}", tip.command, tip.detail));
        }
        lines
    }
}

impl SessionState {
    /// One line: what it was, where, and when.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(title) = &self.title {
            parts.push(title.clone());
        }
        if let Some(branch) = &self.branch {
            parts.push(branch.clone());
        }
        if let Some(when) = &self.when {
            parts.push(when.clone());
        }
        if parts.is_empty() {
            "restored".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

impl MemoryState {
    /// What is worth saying about memory, or nothing at all.
    pub fn summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.linked > 0 {
            parts.push(format!("{} fact{}", self.linked, plural(self.linked)));
        }
        if self.pending > 0 {
            parts.push(format!("{} awaiting review", self.pending));
        }
        if self.improvements > 0 {
            parts.push(format!(
                "{} improvement candidate{}",
                self.improvements,
                plural(self.improvements)
            ));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

/// The line under the wordmark.
///
/// Neutral by default. A display name appears only if the profile system has
/// actually been given one — guessing at it from a git config or a username
/// would be a product greeting the operator never asked for.
fn greeting(app: &App, restored: bool) -> String {
    if app.config.models.is_empty() {
        return "No models configured yet.".into();
    }
    let named = app.read_ui_state(|state| state.profile_name.clone());
    greeting_line(Some(named.as_str()), restored)
}

/// Separated from the reads so the naming rule is testable: the profile name is
/// used only when the operator actually created a profile. `default` is the
/// placeholder every install starts with, and greeting someone as "default"
/// is worse than not greeting them at all.
fn greeting_line(profile: Option<&str>, restored: bool) -> String {
    let name = profile
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("default"));
    match (name, restored) {
        (Some(name), true) => format!("Welcome back, {name}."),
        (Some(name), false) => format!("Welcome, {name}."),
        (None, true) => "Welcome back.".into(),
        (None, false) => "Workspace ready.".into(),
    }
}

/// Sandbox isolation, plus the permission mode when it is not the default.
fn access_line(app: &App) -> String {
    let network = match app.config.sandbox.network.as_str() {
        "off" | "none" => nexus_sandbox::NetworkMode::Off,
        "full" => nexus_sandbox::NetworkMode::Full,
        _ => nexus_sandbox::NetworkMode::Restricted,
    };
    let level = app.sandbox.backend().isolation(network).level;
    let mode = crate::services::permission_mode(&app.config.policy);
    if mode == "default" {
        level.to_string()
    } else {
        format!("{level} · {mode}")
    }
}

fn session_state(app: &App) -> Option<SessionState> {
    let session = app.read_ui_state(|state| state.last_session.clone())?;
    let meta = app.sessions().get(&session).ok()?;
    let title = Some(meta.title.trim())
        .filter(|title| !title.is_empty())
        .map(|title| title.chars().take(48).collect::<String>());
    Some(SessionState {
        title,
        branch: crate::gitx::branch(&app.workspace),
        when: concise_date(&meta.created_at),
        resumable: crate::services::resume_candidates(app)
            .map(|candidates| !candidates.is_empty())
            .unwrap_or(false),
    })
}

fn memory_state(app: &App) -> Option<MemoryState> {
    let memories = app.harness().memories(None, None, true, 10_000).ok()?;
    let linked = memories
        .iter()
        .filter(|record| record.status == nexus_core::harness::MemoryStatus::Active)
        .count();
    let pending = memories
        .iter()
        .filter(|record| record.status == nexus_core::harness::MemoryStatus::Candidate)
        .count();
    let improvements = app
        .harness()
        .workspace_repository()
        .improvement_proposals(None)
        .map(|proposals| {
            proposals
                .iter()
                .filter(|proposal| crate::status::awaits_human(proposal.status))
                .count()
        })
        .unwrap_or(0);
    let state = MemoryState {
        linked,
        pending,
        improvements,
    };
    state.summary().is_some().then_some(state)
}

/// Subsystems that came up degraded.
///
/// Only things the operator can act on. A missing model configuration blocks
/// everything, so it is the one `Blocked` case a normal start can produce.
fn notices(app: &App) -> Vec<BootNotice> {
    let mut notices = Vec::new();
    if app.config.models.is_empty() {
        notices.push(BootNotice {
            subsystem: "MODELS".into(),
            level: NoticeLevel::Blocked,
            detail: "none configured — /setup gets you talking to an agent".into(),
        });
    }
    if !app.config.memory.enabled {
        notices.push(BootNotice {
            subsystem: "MEMORY".into(),
            level: NoticeLevel::Degraded,
            detail: "disabled in config; nothing is carried between sessions".into(),
        });
    }
    for note in app.sandbox_notes.iter().take(1) {
        notices.push(BootNotice {
            subsystem: "SANDBOX".into(),
            level: NoticeLevel::Degraded,
            detail: note.clone(),
        });
    }
    notices
}

/// Two or three commands chosen from what is actually true right now.
///
/// Every tip is conditional on real state, so an operator with no session never
/// reads about `/resume` and one with nothing pending never reads about
/// `/memory`. Three is the ceiling: a tip list nobody finishes reading is
/// decoration.
fn tips(app: &App, session: Option<&SessionState>, memory: Option<&MemoryState>) -> Vec<Tip> {
    let mut tips = Vec::new();
    let mut add = |command: &str, detail: &str| {
        if tips.len() < MAX_TIPS {
            tips.push(Tip {
                command: command.to_string(),
                detail: detail.to_string(),
            });
        }
    };
    if app.config.models.is_empty() {
        add("/setup", "detect a local model and write a config");
        add("/help", "the full command surface");
        return tips;
    }
    if session.is_some_and(|session| session.resumable) {
        add("/resume", "continue the restored session");
    }
    if memory.is_some_and(|memory| memory.pending > 0) {
        add("/memory", "review what the agent recorded");
    }
    if memory.is_some_and(|memory| memory.improvements > 0) {
        add("/rsi", "review self-improvement candidates");
    }
    // `/goal` reads differently depending on whether one exists. Pointing at
    // "the objective in progress" on a fresh workspace would be a tip about
    // something that is not there — the exact invention the panel avoids
    // everywhere else.
    if crate::services::active_goal_id(app).is_some() {
        add("/goal", "pick up the objective in progress");
    } else {
        add("/goal", "define a persistent, verified objective");
    }
    add("/plan", "approve the approach before any change");
    add("/help", "keys, commands, and what they do");
    tips
}

/// Three is enough to read at a glance and small enough to fit a phone.
const MAX_TIPS: usize = 3;

/// `/home/sans/project` → `~/project`, keeping the repository name whatever
/// happens. The panel truncates from the left if it still does not fit, because
/// the last segment is the one that identifies the workspace.
fn display_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && path.starts_with(&home) {
        return format!("~{}", &path[home.len()..]);
    }
    path.to_string()
}

/// `2026-07-23T18:25:53.941Z` → `23 Jul 2026`.
///
/// A boot line is read at a glance; sub-second precision on "when was I last
/// here" is machine detail that happens to be human-readable.
fn concise_date(stamp: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = stamp.split('T').next()?;
    let mut parts = date.split('-');
    let year: u32 = parts.next()?.parse().ok()?;
    let month: usize = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    let name = MONTHS.get(month.checked_sub(1)?)?;
    Some(format!("{day} {name} {year}"))
}

/// The headline for the running version, shown once per version.
///
/// Read from the changelog compiled into the binary, so it cannot drift from
/// what actually shipped and cannot claim a feature this build does not have.
fn whats_new(app: &App) -> Option<String> {
    let version = nexus_core::brand::VERSION;
    let seen = app.read_ui_state(|state| state.last_seen_version.clone());
    if seen == version {
        return None;
    }
    changelog_headline(CHANGELOG, version)
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// First bolded headline under `## [version]`, e.g. `- **Thing happened.**`.
///
/// Returns `None` when the version has no section — a build from an unreleased
/// working tree says nothing rather than showing the previous release's news.
fn changelog_headline(changelog: &str, version: &str) -> Option<String> {
    let heading = format!("## [{version}]");
    let section = changelog.split(&heading).nth(1)?;
    let section = section.split("\n## ").next().unwrap_or(section);
    let mut lines = section.lines();
    let rest = loop {
        let line = lines.next()?.trim();
        if let Some(rest) = line.strip_prefix("- **") {
            break rest;
        }
    };
    // The headline is a markdown bold span, and markdown wraps: reading only
    // the first physical line produced "…is now a real flagship agent — the
    // default Recursive", a sentence cut where the paragraph happened to wrap.
    // Collect until the closing `**`, wherever it lands.
    let mut headline = String::new();
    let mut rest = rest;
    loop {
        match rest.split_once("**") {
            Some((head, _)) => {
                push_word_joined(&mut headline, head);
                break;
            }
            None => {
                push_word_joined(&mut headline, rest);
                rest = match lines.next() {
                    Some(line) => line.trim(),
                    // Unterminated bold: take what there is rather than nothing.
                    None => break,
                };
            }
        }
    }
    let headline = headline.trim().trim_end_matches(['.', ':', ',']);
    if headline.is_empty() {
        return None;
    }
    // A boot line is a glance. Changelog headlines are written to be read in a
    // document and routinely run past a terminal row, so this one is cut at a
    // word boundary rather than allowed to wrap the whole panel.
    Some(truncate_words(headline, MAX_HEADLINE_CHARS))
}

const MAX_HEADLINE_CHARS: usize = 64;

fn push_word_joined(target: &mut String, piece: &str) {
    let piece = piece.trim();
    if piece.is_empty() {
        return;
    }
    if !target.is_empty() {
        target.push(' ');
    }
    target.push_str(piece);
}

/// Cut to `max` characters on a word boundary, with an ellipsis when anything
/// was dropped. Cutting mid-word is how "the default" became "the def ault".
fn truncate_words(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut kept = String::new();
    for word in text.split_whitespace() {
        let projected = if kept.is_empty() {
            word.chars().count()
        } else {
            kept.chars().count() + 1 + word.chars().count()
        };
        if projected > max.saturating_sub(1) {
            break;
        }
        if !kept.is_empty() {
            kept.push(' ');
        }
        kept.push_str(word);
    }
    if kept.is_empty() {
        kept = text.chars().take(max.saturating_sub(1)).collect();
    }
    format!("{}…", kept.trim_end_matches([',', ';', '—', '-']).trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Changelog

## [2.11.0] — 2026-07-30

### Added

- **Governed self-improvement.** Long body text that should not be shown.
- **Something else.** More text.

## [2.10.0] — 2026-07-24

- **Older news.** Not this one.
";

    #[test]
    fn the_headline_comes_from_the_running_version_only() {
        assert_eq!(
            changelog_headline(SAMPLE, "2.11.0").as_deref(),
            Some("Governed self-improvement")
        );
        assert_eq!(
            changelog_headline(SAMPLE, "2.10.0").as_deref(),
            Some("Older news")
        );
    }

    #[test]
    fn an_unknown_version_produces_no_headline() {
        assert_eq!(changelog_headline(SAMPLE, "9.9.9"), None);
        assert_eq!(changelog_headline("", "2.11.0"), None);
    }

    /// The shipped changelog has to actually parse — otherwise the stage would
    /// silently vanish for every real build.
    #[test]
    fn the_shipped_changelog_yields_a_headline_for_this_build() {
        let headline = changelog_headline(CHANGELOG, nexus_core::brand::VERSION)
            .expect("the running version needs a changelog section");
        assert!(!headline.is_empty());
        assert!(!headline.contains("**"), "{headline}");
    }

    /// The bug this pins: a markdown bold span that wraps across source lines
    /// was read one physical line at a time, so the panel showed a sentence cut
    /// mid-word — "…agent — the def" + "ault Recursive".
    #[test]
    fn a_headline_that_wraps_in_the_source_is_rejoined_not_cut() {
        let wrapped = "\
## [9.0.0]

- **`nexus` is now a real flagship agent — the default Recursive
  Self-Improvement (RSI) agent.** Body text.
";
        let headline = changelog_headline(wrapped, "9.0.0").expect("headline");
        assert!(!headline.contains("  "), "{headline}");
        // Whatever survives the cap must end on a word, not inside one.
        assert!(!headline.contains("def…"), "{headline}");
        assert!(headline.chars().count() <= MAX_HEADLINE_CHARS, "{headline}");
    }

    /// The wake flow is a glance; a changelog headline is written for a
    /// document and routinely outruns a terminal row.
    #[test]
    fn a_long_headline_is_cut_at_a_word_boundary() {
        let long = format!(
            "## [9.0.0]\n\n- **{}.** body\n",
            "alpha beta gamma delta ".repeat(20)
        );
        let headline = changelog_headline(&long, "9.0.0").expect("headline");
        assert!(headline.chars().count() <= MAX_HEADLINE_CHARS, "{headline}");
        assert!(headline.ends_with('…'));
        // Every word before the ellipsis is whole.
        for word in headline.trim_end_matches('…').split_whitespace() {
            assert!(
                ["alpha", "beta", "gamma", "delta"].contains(&word),
                "cut mid-word: {headline}"
            );
        }
    }

    #[test]
    fn a_raw_timestamp_becomes_a_date_a_person_reads() {
        assert_eq!(
            concise_date("2026-07-23T18:25:53.941Z").as_deref(),
            Some("23 Jul 2026")
        );
        assert_eq!(concise_date("2026-01-01").as_deref(), Some("1 Jan 2026"));
        // Garbage in, nothing out — never a half-parsed date.
        assert_eq!(concise_date("not-a-date"), None);
        assert_eq!(concise_date("2026-13-01"), None);
    }

    #[test]
    fn a_home_path_shortens_and_keeps_the_project_name() {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return;
        }
        let shown = display_path(&format!("{home}/code/project"));
        assert_eq!(shown, "~/code/project");
        assert_eq!(display_path("/srv/build"), "/srv/build");
    }

    /// A section with nothing real to say does not exist. The panel must be
    /// able to ask "is there memory?" and get an honest no.
    #[test]
    fn an_empty_memory_state_has_no_summary() {
        let empty = MemoryState {
            linked: 0,
            pending: 0,
            improvements: 0,
        };
        assert_eq!(empty.summary(), None);
        let one = MemoryState {
            linked: 1,
            pending: 0,
            improvements: 0,
        };
        assert_eq!(one.summary().as_deref(), Some("1 fact"));
        let many = MemoryState {
            linked: 3,
            pending: 2,
            improvements: 1,
        };
        assert_eq!(
            many.summary().as_deref(),
            Some("3 facts · 2 awaiting review · 1 improvement candidate")
        );
    }

    #[test]
    fn a_session_summary_omits_what_it_does_not_know() {
        let bare = SessionState {
            title: None,
            branch: None,
            when: None,
            resumable: false,
        };
        assert_eq!(bare.summary(), "restored");
        let full = SessionState {
            title: Some("fix the tier check".into()),
            branch: Some("main".into()),
            when: Some("23 Jul 2026".into()),
            resumable: true,
        };
        assert_eq!(full.summary(), "fix the tier check · main · 23 Jul 2026");
    }

    #[test]
    fn the_collapsed_header_names_the_agent_and_model() {
        let snapshot = BootSnapshot {
            agent: "reviewer".into(),
            model: "Codex / gpt-5.6".into(),
            session: Some(SessionState {
                title: None,
                branch: Some("main".into()),
                when: None,
                resumable: true,
            }),
            ..Default::default()
        };
        let compact = snapshot.compact();
        assert!(compact.contains("reviewer"), "{compact}");
        assert!(compact.contains("Codex / gpt-5.6"), "{compact}");
        assert!(compact.contains("main"), "{compact}");
        assert!(compact.contains("restored"), "{compact}");
    }

    /// The non-TTY form carries the same facts with no decoration at all.
    #[test]
    fn the_plain_form_has_no_borders_or_icons() {
        let snapshot = BootSnapshot {
            version: "2.11.0".into(),
            workspace: "~/project".into(),
            model: "Ollama / qwen".into(),
            agent: "implementer".into(),
            access: "path-validation-only".into(),
            update: Some("Something shipped".into()),
            tips: vec![Tip {
                command: "/resume".into(),
                detail: "continue the restored session".into(),
            }],
            ..Default::default()
        };
        let text = snapshot.plain_lines().join("\n");
        for decoration in ['╭', '│', '╰', '◢', '✓', '◈'] {
            assert!(!text.contains(decoration), "{decoration} in {text}");
        }
        assert!(text.contains("Workspace: ~/project"), "{text}");
        assert!(text.contains("Tip: /resume"), "{text}");
    }

    /// A name appears only when the operator gave the profile system one.
    /// `default` is the placeholder every install ships with.
    #[test]
    fn the_greeting_never_invents_or_repeats_a_placeholder_name() {
        assert_eq!(greeting_line(Some("Sans"), true), "Welcome back, Sans.");
        assert_eq!(greeting_line(Some("Sans"), false), "Welcome, Sans.");
        for placeholder in [None, Some(""), Some("  "), Some("default"), Some("Default")] {
            assert_eq!(greeting_line(placeholder, true), "Welcome back.");
            assert_eq!(greeting_line(placeholder, false), "Workspace ready.");
        }
    }

    #[test]
    fn plurals_read_correctly() {
        assert_eq!(format!("1 fact{}", plural(1)), "1 fact");
        assert_eq!(format!("2 fact{}", plural(2)), "2 facts");
    }
}
