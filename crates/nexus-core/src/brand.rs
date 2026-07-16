//! Canonical NEXUS identity and responsive terminal lockups.
//!
//! This module owns the product geometry, copy, spacing, color rules, and
//! Unicode/ASCII fallbacks. Renderers only translate semantic roles into
//! their terminal framework's style type.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const PRODUCT: &str = "NEXUS";
pub const COMPANY: &str = "Silent Protocol";
pub const PRODUCT_FULL: &str = "NEXUS by Silent Protocol";
pub const MARK: &str = "NEXUS";
pub const COMPACT_MARK: &str = "NEXUS";
pub const CLI: &str = "snx";
pub const ATTRIBUTION: &str = "by Silent Protocol";
pub const WORDMARK: &str = "N  E  X  U  S";
pub const TAGLINE: &str = "LOCAL INTELLIGENCE. CONTROLLED EXECUTION.";
pub const TAGLINE_FIRST: &str = "LOCAL INTELLIGENCE.";
pub const TAGLINE_SECOND: &str = "CONTROLLED EXECUTION.";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const FULL_MIN_WIDTH: u16 = 42;
const FULL_MIN_HEIGHT: u16 = 18;

const ICON_UNICODE: [&str; 6] = [
    "██▄       ██",
    "████▄     ██",
    "██ ▀██▄   ██",
    "██   ▀██▄ ██",
    "██     ▀████",
    "██       ▀██",
];

const ICON_ASCII: [&str; 6] = [
    "##        ##",
    "####      ##",
    "## ##     ##",
    "##   ##   ##",
    "##     ## ##",
    "##       ###",
];

const WORDMARK_UNICODE: [&str; 5] = [
    "█▄  █  █████  █   █  █   █  █████",
    "██▄ █  █       █ █   █   █  █    ",
    "█ █▄█  ████     █    █   █  █████",
    "█  ██  █       █ █   █   █      █",
    "█  ▀█  █████  █   █  █████  █████",
];

const WORDMARK_ASCII: [&str; 5] = [
    "##  #  #####  #   #  #   #  #####",
    "### #  #       # #   #   #  #    ",
    "# ###  ####     #    #   #  #####",
    "#  ##  #       # #   #   #      #",
    "#   #  #####  #   #  #####  #####",
];

/// Terminal color capability. Detection is shared by the TUI and CLI so
/// brand output degrades consistently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSupport {
    TrueColor,
    Ansi256,
    Ansi16,
    None,
}

/// Basic ANSI fallback used when a terminal has no extended color support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasicColor {
    Cyan,
    Magenta,
    Green,
    Yellow,
    Red,
    DarkGray,
    White,
}

/// One canonical brand color across true color, 256-color, and ANSI modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrandColor {
    pub rgb: (u8, u8, u8),
    pub ansi256: u8,
    pub ansi16: BasicColor,
}

/// Premium azure: brighter and calmer than the previous fully saturated cyan.
pub const BLUE: BrandColor = BrandColor {
    rgb: (103, 207, 255),
    ansi256: 81,
    ansi16: BasicColor::Cyan,
};

/// High-presence magenta reserved for the NEXUS identity mark.
pub const PINK: BrandColor = BrandColor {
    rgb: (255, 82, 190),
    ansi256: 205,
    ansi16: BasicColor::Magenta,
};

/// Soft neutral used for attribution and supporting copy.
pub const SOFT_GRAY: BrandColor = BrandColor {
    rgb: (154, 164, 188),
    ansi256: 246,
    ansi16: BasicColor::DarkGray,
};

pub const TEXT: BrandColor = BrandColor {
    rgb: (220, 225, 239),
    ansi256: 254,
    ansi16: BasicColor::White,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrandRole {
    Icon,
    Wordmark,
    Attribution,
    Tagline,
    Spacer,
}

impl BrandRole {
    pub fn color(self) -> Option<BrandColor> {
        match self {
            BrandRole::Icon => Some(PINK),
            BrandRole::Wordmark => Some(BLUE),
            BrandRole::Attribution | BrandRole::Tagline => Some(SOFT_GRAY),
            BrandRole::Spacer => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrandVariant {
    Full,
    Compact,
    IconOnly,
    WordmarkOnly,
    Monochrome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrandConstraints {
    pub width: u16,
    pub height: u16,
    pub unicode: bool,
}

impl Default for BrandConstraints {
    fn default() -> Self {
        Self {
            width: 80,
            height: 24,
            unicode: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrandSpan {
    pub text: String,
    pub role: BrandRole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrandLine {
    pub spans: Vec<BrandSpan>,
}

impl BrandLine {
    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    pub fn visible_width(&self) -> usize {
        self.spans
            .iter()
            .map(|span| visible_width(&span.text))
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrandLockup {
    pub variant: BrandVariant,
    pub lines: Vec<BrandLine>,
    pub width: u16,
    pub height: u16,
    pub monochrome: bool,
}

impl BrandLockup {
    pub fn plain_text(&self) -> String {
        self.lines
            .iter()
            .map(BrandLine::text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Build a responsive lockup. Full and monochrome requests automatically use
/// the compact composition when the available terminal area is too small.
pub fn lockup(requested: BrandVariant, constraints: BrandConstraints) -> BrandLockup {
    let width = constraints.width.max(1);
    let height = constraints.height.max(1);
    let monochrome = requested == BrandVariant::Monochrome;
    let resolved = match requested {
        BrandVariant::Full | BrandVariant::Monochrome
            if width < FULL_MIN_WIDTH || height < FULL_MIN_HEIGHT =>
        {
            BrandVariant::Compact
        }
        BrandVariant::Monochrome => BrandVariant::Full,
        other => other,
    };

    let mut lines = match resolved {
        BrandVariant::Full => full_lines(constraints.unicode),
        BrandVariant::Compact => compact_lines(width, constraints.unicode),
        BrandVariant::IconOnly => icon_lines(height, constraints.unicode),
        BrandVariant::WordmarkOnly => wordmark_lines(width, height, constraints.unicode),
        BrandVariant::Monochrome => unreachable!("resolved above"),
    };

    lines = lines
        .into_iter()
        .map(|line| fit_line_to_width(line, width as usize))
        .collect();
    if lines.len() > height as usize {
        lines.truncate(height as usize);
    }
    center_lines(&mut lines);

    BrandLockup {
        variant: resolved,
        width: lines
            .iter()
            .map(BrandLine::visible_width)
            .max()
            .unwrap_or(0) as u16,
        height: lines.len() as u16,
        lines,
        monochrome,
    }
}

fn full_lines(unicode: bool) -> Vec<BrandLine> {
    let icon = if unicode {
        &ICON_UNICODE[..]
    } else {
        &ICON_ASCII[..]
    };
    let wordmark = if unicode {
        &WORDMARK_UNICODE[..]
    } else {
        &WORDMARK_ASCII[..]
    };
    let mut lines = Vec::with_capacity(17);
    lines.extend(
        icon.iter()
            .map(|text| line((*text).to_string(), BrandRole::Icon)),
    );
    lines.push(spacer());
    lines.extend(
        wordmark
            .iter()
            .map(|text| line((*text).to_string(), BrandRole::Wordmark)),
    );
    lines.push(spacer());
    lines.push(line(ATTRIBUTION, BrandRole::Attribution));
    lines.push(spacer());
    lines.push(line(TAGLINE_FIRST, BrandRole::Tagline));
    lines.push(line(TAGLINE_SECOND, BrandRole::Tagline));
    lines
}

fn compact_lines(width: u16, unicode: bool) -> Vec<BrandLine> {
    let inline = if width >= 19 {
        BrandLine {
            spans: vec![
                BrandSpan {
                    text: if unicode { "▚  " } else { ">> " }.to_string(),
                    role: BrandRole::Icon,
                },
                BrandSpan {
                    text: WORDMARK.to_string(),
                    role: BrandRole::Wordmark,
                },
            ],
        }
    } else if width >= 9 {
        BrandLine {
            spans: vec![
                BrandSpan {
                    text: if unicode { "▚ " } else { "> " }.to_string(),
                    role: BrandRole::Icon,
                },
                BrandSpan {
                    text: MARK.to_string(),
                    role: BrandRole::Wordmark,
                },
            ],
        }
    } else {
        line(MARK, BrandRole::Wordmark)
    };
    let attribution = if width >= ATTRIBUTION.len() as u16 {
        ATTRIBUTION
    } else if width >= 10 {
        "by Silent"
    } else {
        "by SP"
    };
    let mut lines = vec![inline, line(attribution, BrandRole::Attribution)];
    if width >= TAGLINE.len() as u16 {
        lines.push(line(TAGLINE, BrandRole::Tagline));
    } else {
        lines.push(line(TAGLINE_FIRST, BrandRole::Tagline));
        lines.push(line(TAGLINE_SECOND, BrandRole::Tagline));
    }
    lines
}

fn icon_lines(height: u16, unicode: bool) -> Vec<BrandLine> {
    if height < ICON_UNICODE.len() as u16 {
        return vec![line(if unicode { "▚" } else { ">" }, BrandRole::Icon)];
    }
    let icon = if unicode {
        &ICON_UNICODE[..]
    } else {
        &ICON_ASCII[..]
    };
    icon.iter()
        .map(|text| line((*text).to_string(), BrandRole::Icon))
        .collect()
}

fn wordmark_lines(width: u16, height: u16, unicode: bool) -> Vec<BrandLine> {
    if width >= 37 && height >= 5 {
        let wordmark = if unicode {
            &WORDMARK_UNICODE[..]
        } else {
            &WORDMARK_ASCII[..]
        };
        return wordmark
            .iter()
            .map(|text| line((*text).to_string(), BrandRole::Wordmark))
            .collect();
    }
    vec![line(
        if width >= 13 { WORDMARK } else { MARK },
        BrandRole::Wordmark,
    )]
}

fn spacer() -> BrandLine {
    BrandLine { spans: Vec::new() }
}

fn center_lines(lines: &mut [BrandLine]) {
    let width = lines
        .iter()
        .map(BrandLine::visible_width)
        .max()
        .unwrap_or(0);
    for line in lines {
        if line.spans.is_empty() {
            continue;
        }
        let pad = width.saturating_sub(line.visible_width()) / 2;
        if pad > 0 {
            line.spans.insert(
                0,
                BrandSpan {
                    text: " ".repeat(pad),
                    role: BrandRole::Spacer,
                },
            );
        }
    }
}

fn line(text: impl Into<String>, role: BrandRole) -> BrandLine {
    BrandLine {
        spans: vec![BrandSpan {
            text: text.into(),
            role,
        }],
    }
}

fn fit_line_to_width(mut line: BrandLine, max_width: usize) -> BrandLine {
    let mut remaining = max_width;
    for span in &mut line.spans {
        span.text = fit_to_width(&span.text, remaining);
        remaining = remaining.saturating_sub(visible_width(&span.text));
    }
    line.spans.retain(|span| !span.text.is_empty());
    line
}

fn fit_to_width(text: &str, max_width: usize) -> String {
    let mut width = 0usize;
    text.chars()
        .take_while(|ch| {
            let char_width = UnicodeWidthChar::width(*ch).unwrap_or(0);
            if width + char_width > max_width {
                false
            } else {
                width += char_width;
                true
            }
        })
        .collect()
}

/// Visible terminal-cell width. ANSI CSI/OSC control sequences do not
/// contribute, which keeps centering correct for pre-styled strings too.
pub fn visible_width(text: &str) -> usize {
    UnicodeWidthStr::width(strip_ansi(text).as_str())
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                let mut escaped = false;
                for c in chars.by_ref() {
                    if c == '\u{7}' || (escaped && c == '\\') {
                        break;
                    }
                    escaped = c == '\x1b';
                }
            }
            Some(_) | None => {}
        }
    }
    out
}

/// Conservative Unicode capability check with an explicit escape hatch for
/// terminals or fonts that cannot render block glyphs reliably.
pub fn unicode_supported() -> bool {
    if std::env::var_os("SNX_ASCII").is_some() {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
        return false;
    }
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok())
        .unwrap_or_default();
    !matches!(locale.as_str(), "C" | "POSIX")
}

pub fn detect_color_support(no_color: bool, terminal: bool) -> ColorSupport {
    if no_color || !terminal || std::env::var_os("NO_COLOR").is_some() {
        return ColorSupport::None;
    }
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        return ColorSupport::TrueColor;
    }
    let term = std::env::var("TERM").unwrap_or_default();
    if term.contains("256color") {
        return ColorSupport::Ansi256;
    }
    if term == "dumb" || term.is_empty() {
        return ColorSupport::None;
    }
    ColorSupport::Ansi16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_terminal_sizes_fit_without_clipping() {
        for (width, height) in [(60, 20), (80, 24), (100, 30), (120, 40), (160, 50)] {
            let lockup = lockup(
                BrandVariant::Full,
                BrandConstraints {
                    width,
                    height,
                    unicode: true,
                },
            );
            assert_eq!(lockup.variant, BrandVariant::Full);
            assert!(lockup.width <= width);
            assert!(lockup.height <= height);
            assert!(lockup
                .lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.role == BrandRole::Wordmark && span.text.contains("█████")));
        }
    }

    #[test]
    fn short_or_narrow_terminals_use_compact_lockup() {
        for (width, height) in [(40, 24), (80, 12), (20, 8)] {
            let lockup = lockup(
                BrandVariant::Full,
                BrandConstraints {
                    width,
                    height,
                    unicode: true,
                },
            );
            assert_eq!(lockup.variant, BrandVariant::Compact);
            assert!(lockup.width <= width);
            assert!(lockup.height <= height);
            assert!(lockup.plain_text().replace(' ', "").contains("NEXUS"));
            assert!(!lockup.plain_text().contains("SILENT//"));
        }
    }

    #[test]
    fn ascii_fallback_contains_only_ascii() {
        let lockup = lockup(
            BrandVariant::Full,
            BrandConstraints {
                unicode: false,
                ..BrandConstraints::default()
            },
        );
        assert!(lockup.plain_text().is_ascii());
        assert!(lockup.plain_text().contains("NEXUS") || lockup.plain_text().contains("#####"));
    }

    #[test]
    fn ansi_sequences_do_not_change_visible_width() {
        assert_eq!(visible_width("\x1b[38;2;1;2;3mNEXUS\x1b[0m"), 5);
        assert_eq!(visible_width("\x1b]0;title\u{7}N  E"), 4);
    }

    #[test]
    fn hierarchy_keeps_company_secondary() {
        let lockup = lockup(BrandVariant::Full, BrandConstraints::default());
        let wordmark_rows = lockup
            .lines
            .iter()
            .filter(|line| {
                line.spans
                    .iter()
                    .any(|span| span.role == BrandRole::Wordmark)
            })
            .count();
        let attribution_rows = lockup
            .lines
            .iter()
            .filter(|line| {
                line.spans
                    .iter()
                    .any(|span| span.role == BrandRole::Attribution)
            })
            .count();
        assert_eq!(wordmark_rows, 5);
        assert_eq!(attribution_rows, 1);
        assert!(lockup.plain_text().contains(ATTRIBUTION));
    }
}
