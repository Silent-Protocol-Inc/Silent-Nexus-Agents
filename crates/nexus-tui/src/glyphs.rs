//! Marks for tool families in the activity timeline.
//!
//! A tool row is read at a glance, and the family — is this reading, writing,
//! running something? — is the part worth encoding in one cell. The names are
//! matched rather than enumerated because tools arrive from the registry, from
//! MCP servers, and from custom agents; an unknown name gets the neutral mark
//! instead of no mark at all.
//!
//! Three tiers, because the terminals this project supports do not agree on
//! what they can draw. The geometric tier is the default: it is single-width
//! everywhere, it matches the `✓ ✕ ◆ ◇ △` marks the timeline already uses, and
//! it does not depend on an emoji font being installed. Emoji are opt-in — they
//! are double-width and render as boxes on several mobile clients.

/// Which set of marks to draw with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GlyphTier {
    /// Opt-in via `[tui.activity].tool_icons = "emoji"`.
    Emoji,
    /// The default wherever the terminal can draw beyond ASCII.
    #[default]
    Geometric,
    /// `SNX_ASCII`, `TERM=dumb`, or a `C`/`POSIX` locale.
    Ascii,
}

impl GlyphTier {
    /// Resolve the tier from the configured preference and the terminal.
    ///
    /// A terminal that cannot draw Unicode wins over any preference: honoring
    /// `tool_icons = "emoji"` there would print replacement boxes down the
    /// whole timeline.
    pub fn resolve(preference: &str, unicode_supported: bool) -> Self {
        if !unicode_supported {
            return Self::Ascii;
        }
        match preference.trim() {
            "emoji" => Self::Emoji,
            "ascii" => Self::Ascii,
            _ => Self::Geometric,
        }
    }
}

/// The family a tool belongs to, as far as the operator is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolFamily {
    Read,
    Write,
    Search,
    Exec,
    Git,
    Net,
    Memory,
    Plan,
    Agent,
    Other,
}

/// Classify a tool by name.
///
/// Ordered so the more specific families win: `repo.git_status` is git rather
/// than search, and `memory.add` is memory rather than write. Names are split
/// into words first and matched whole — a substring test classified
/// `acme.frobnicate` as a read, because "frobni**cat**e" contains `cat`. Words
/// keep tools this build has never seen (MCP servers, custom agents) landing
/// somewhere sensible without inventing matches inside longer ones.
pub fn family(tool: &str) -> ToolFamily {
    let name = tool.to_ascii_lowercase();
    let words: Vec<&str> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    // Exact words, plus a prefix match for needles long enough that a false
    // positive is implausible — so `readFile` still reads as a read, while the
    // three-letter needles (`cat`, `ls`, `mv`, `net`) stay exact.
    let has = |needles: &[&str]| {
        needles.iter().any(|needle| {
            words
                .iter()
                .any(|word| word == needle || (needle.len() >= 4 && word.starts_with(needle)))
        })
    };
    if has(&["git", "branch", "commit", "diff"]) {
        ToolFamily::Git
    } else if has(&["memory", "recall", "remember"]) {
        ToolFamily::Memory
    } else if has(&["plan", "stage", "task"]) {
        ToolFamily::Plan
    } else if has(&["agent", "delegate", "subagent"]) {
        ToolFamily::Agent
    } else if has(&["http", "fetch", "web", "url", "net"]) {
        ToolFamily::Net
    } else if has(&["search", "grep", "find", "structure", "index"]) {
        ToolFamily::Search
    } else if has(&["write", "edit", "patch", "create", "delete", "remove", "mv"]) {
        ToolFamily::Write
    } else if has(&["shell", "exec", "run", "command", "bash", "test", "build"]) {
        ToolFamily::Exec
    } else if has(&["read", "open", "cat", "list", "dir", "ls", "stat"]) {
        ToolFamily::Read
    } else {
        ToolFamily::Other
    }
}

impl ToolFamily {
    /// The mark for this family in the given tier.
    ///
    /// Every geometric and ASCII mark is one cell wide, so a row's alignment
    /// does not depend on which tool ran. Emoji are two cells and callers must
    /// measure with `unicode_width` rather than counting chars.
    pub fn glyph(self, tier: GlyphTier) -> &'static str {
        match tier {
            GlyphTier::Emoji => match self {
                Self::Read => "📄",
                Self::Write => "✏️",
                Self::Search => "🔎",
                Self::Exec => "⚙️",
                Self::Git => "🌿",
                Self::Net => "🌐",
                Self::Memory => "🧠",
                Self::Plan => "🗺️",
                Self::Agent => "🤖",
                Self::Other => "🔹",
            },
            GlyphTier::Geometric => match self {
                Self::Read => "▤",
                Self::Write => "▨",
                Self::Search => "⌕",
                Self::Exec => "⏵",
                Self::Git => "⑂",
                Self::Net => "◈",
                Self::Memory => "⌸",
                Self::Plan => "⌗",
                Self::Agent => "⟁",
                Self::Other => "·",
            },
            GlyphTier::Ascii => match self {
                Self::Read => "r",
                Self::Write => "w",
                Self::Search => "s",
                Self::Exec => "x",
                Self::Git => "g",
                Self::Net => "n",
                Self::Memory => "m",
                Self::Plan => "p",
                Self::Agent => "a",
                Self::Other => "-",
            },
        }
    }
}

/// The mark for a tool by name, in one call.
pub fn tool_glyph(tool: &str, tier: GlyphTier) -> &'static str {
    family(tool).glyph(tier)
}

static TIER: std::sync::OnceLock<GlyphTier> = std::sync::OnceLock::new();

/// Fix the tier for this process from the configured preference.
///
/// Resolved once at start-up rather than per row: the terminal's capabilities
/// do not change mid-session, and the render path runs on every frame.
pub fn configure(preference: &str) {
    let _ = TIER.set(GlyphTier::resolve(
        preference,
        nexus_core::brand::unicode_supported(),
    ));
}

/// The tier in force. Defaults to the geometric set — safe in any terminal
/// that can draw Unicode — when nothing configured one.
pub fn tier() -> GlyphTier {
    *TIER.get_or_init(|| GlyphTier::resolve("geometric", nexus_core::brand::unicode_supported()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn specific_families_win_over_general_ones() {
        assert_eq!(family("repo.git_status"), ToolFamily::Git);
        assert_eq!(family("memory.add"), ToolFamily::Memory);
        assert_eq!(family("fs.read_file"), ToolFamily::Read);
        assert_eq!(family("fs.write_file"), ToolFamily::Write);
        assert_eq!(family("repo.structure"), ToolFamily::Search);
        assert_eq!(family("shell.exec"), ToolFamily::Exec);
        assert_eq!(family("plan.submit"), ToolFamily::Plan);
    }

    #[test]
    fn camel_case_names_are_classified_too() {
        // A prefix match, not a substring one — MCP servers name tools freely.
        assert_eq!(family("fs.readFile"), ToolFamily::Read);
        assert_eq!(family("fs.writeFile"), ToolFamily::Write);
        assert_eq!(family("repo.searchIndex"), ToolFamily::Search);
    }

    #[test]
    fn an_unknown_tool_still_gets_a_mark() {
        // MCP servers and custom agents contribute names this build never saw.
        // `frobnicate` contains `cat`, which a substring match classified as a
        // read; this is the case that forced word matching.
        assert_eq!(family("acme.frobnicate"), ToolFamily::Other);
        assert!(!tool_glyph("acme.frobnicate", GlyphTier::Geometric).is_empty());
        assert!(!tool_glyph("acme.frobnicate", GlyphTier::Ascii).is_empty());
    }

    #[test]
    fn geometric_and_ascii_marks_are_one_cell_wide() {
        // Row alignment must not depend on which tool ran.
        for tier in [GlyphTier::Geometric, GlyphTier::Ascii] {
            for tool in [
                "fs.read_file",
                "fs.write_file",
                "repo.structure",
                "shell.exec",
                "repo.git_status",
                "http.fetch",
                "memory.add",
                "plan.submit",
                "agent.delegate",
                "acme.frobnicate",
            ] {
                let glyph = tool_glyph(tool, tier);
                assert_eq!(glyph.width(), 1, "{tool} in {tier:?} is not one cell");
            }
        }
    }

    #[test]
    fn a_terminal_without_unicode_overrides_the_preference() {
        // Honoring "emoji" on a `C`-locale terminal prints boxes, not icons.
        assert_eq!(GlyphTier::resolve("emoji", false), GlyphTier::Ascii);
        assert_eq!(GlyphTier::resolve("emoji", true), GlyphTier::Emoji);
        assert_eq!(GlyphTier::resolve("", true), GlyphTier::Geometric);
        assert_eq!(GlyphTier::resolve("nonsense", true), GlyphTier::Geometric);
    }
}
