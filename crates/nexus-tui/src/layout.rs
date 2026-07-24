//! Centralized responsive-layout system for the TUI.
//!
//! One place classifies the terminal into width/height breakpoints and derives
//! how many rows the header, status bar, and input may use, whether the sidebar
//! is shown, and which label form the chrome should use. Header, footer, and
//! input all consume [`ResponsiveLayout`] instead of scattering width checks.
//!
//! Everything here is theme-agnostic (no ratatui `Style`): renderers translate
//! the semantic [`SegColor`] into their theme. Widths are measured with
//! `nexus_core::brand::visible_width` so wide glyphs and any stray control
//! sequences never break the arithmetic.

use nexus_core::brand::visible_width;
use ratatui::layout::Rect;

/// Horizontal breakpoints (columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidthClass {
    Wide,    // >= 120
    Desktop, // 90..=119
    Compact, // 65..=89
    Narrow,  // 45..=64
    Mobile,  // < 45
}

/// Vertical breakpoints (rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightClass {
    Tall,      // >= 30
    Standard,  // 22..=29
    Short,     // 16..=21
    VeryShort, // < 16
}

/// Which label form the chrome should render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelForm {
    Full,
    Compact,
    Minimal,
}

impl WidthClass {
    pub fn from_width(width: u16) -> Self {
        match width {
            w if w >= 120 => WidthClass::Wide,
            w if w >= 90 => WidthClass::Desktop,
            w if w >= 65 => WidthClass::Compact,
            w if w >= 45 => WidthClass::Narrow,
            _ => WidthClass::Mobile,
        }
    }

    pub fn label_form(self) -> LabelForm {
        match self {
            WidthClass::Wide => LabelForm::Full,
            WidthClass::Desktop | WidthClass::Compact => LabelForm::Compact,
            WidthClass::Narrow | WidthClass::Mobile => LabelForm::Minimal,
        }
    }
}

impl HeightClass {
    pub fn from_height(height: u16) -> Self {
        match height {
            h if h >= 30 => HeightClass::Tall,
            h if h >= 22 => HeightClass::Standard,
            h if h >= 16 => HeightClass::Short,
            _ => HeightClass::VeryShort,
        }
    }
}

/// Derived layout budget shared by every major TUI section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponsiveLayout {
    pub width_class: WidthClass,
    pub height_class: HeightClass,
    pub show_sidebar: bool,
    pub sidebar_width: u16,
    pub header_rows: u16,
    pub status_rows: u16,
    pub input_max_rows: u16,
    /// Rows the pinned plan tracker may take above the composer, once the rest
    /// of the chrome has been paid for. Zero when the terminal is too short to
    /// spend any: the conversation and the input matter more than the tracker.
    pub plan_panel_max_rows: u16,
    pub compact_labels: bool,
    pub too_small: bool,
}

impl ResponsiveLayout {
    pub fn label_form(&self) -> LabelForm {
        self.width_class.label_form()
    }
}

/// Minimum conversation width kept when the sidebar is shown.
const MIN_CONVERSATION_WIDTH: u16 = 50;
/// Sidebar width when shown.
const SIDEBAR_WIDTH: u16 = 36;

/// Classify a terminal area into a full responsive budget.
pub fn classify(area: Rect) -> ResponsiveLayout {
    let width_class = WidthClass::from_width(area.width);
    let height_class = HeightClass::from_height(area.height);

    let mut header_rows = match width_class {
        WidthClass::Wide | WidthClass::Desktop => 1,
        WidthClass::Compact => 2,
        WidthClass::Narrow | WidthClass::Mobile => 3,
    };
    let mut status_rows = match width_class {
        WidthClass::Wide | WidthClass::Desktop => 1,
        WidthClass::Compact | WidthClass::Narrow => 2,
        WidthClass::Mobile => 3,
    };
    // Height pressure: never spend rows we do not have.
    match height_class {
        HeightClass::VeryShort => {
            header_rows = 1;
            status_rows = 1;
        }
        HeightClass::Short => {
            header_rows = header_rows.min(2);
            status_rows = status_rows.min(2);
        }
        _ => {}
    }

    let input_max_rows = match (width_class, height_class) {
        (_, HeightClass::VeryShort) => 1,
        (WidthClass::Mobile, _) => 3,
        _ => 4,
    };

    let roomy_width = matches!(width_class, WidthClass::Wide | WidthClass::Desktop);
    let roomy_height = matches!(height_class, HeightClass::Tall | HeightClass::Standard);
    let show_sidebar =
        roomy_width && roomy_height && area.width >= MIN_CONVERSATION_WIDTH + SIDEBAR_WIDTH;

    // Three lines is the smallest useful tracker (title, active step, and one
    // either side); seven is enough to show a window of a long plan without
    // crowding the conversation.
    let plan_panel_max_rows = match height_class {
        HeightClass::VeryShort => 0,
        HeightClass::Short => 3,
        HeightClass::Standard => 5,
        HeightClass::Tall => 7,
    };

    ResponsiveLayout {
        width_class,
        height_class,
        show_sidebar,
        sidebar_width: SIDEBAR_WIDTH,
        header_rows,
        status_rows,
        input_max_rows,
        plan_panel_max_rows,
        compact_labels: width_class != WidthClass::Wide,
        too_small: area.width < 24 || area.height < 8,
    }
}

/// Intentional compact representation of a sandbox level; the full value stays
/// available in `/status` and the Ctrl+S status modal.
pub fn sandbox_short(level: &str) -> &str {
    match level {
        "path-validation-only" => "path-only",
        "restricted-local-process" => "restricted",
        "container-isolated" => "container",
        "namespace-isolated" => "namespace",
        "disabled" => "off",
        other => other,
    }
}

/// Width-aware path compaction. `$HOME` becomes `~`; when `project_only` (very
/// narrow), only the final component is kept. Otherwise the identifying tail is
/// preserved and the head is elided with `…`. Uses display width, never bytes,
/// and never splits inside a character.
pub fn compact_path(path: &str, max: usize, project_only: bool) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let normalized = if !home.is_empty() && path.starts_with(&home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    };

    if project_only {
        let leaf = normalized
            .trim_end_matches('/')
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or(&normalized);
        return truncate_display(leaf, max);
    }

    if visible_width(&normalized) <= max {
        return normalized;
    }
    if max <= 1 {
        return truncate_display(&normalized, max);
    }
    // Keep the tail (where you are); elide the head with a leading `…`.
    let budget = max - 1;
    let tail = take_last_display(&normalized, budget);
    format!("…{tail}")
}

/// Truncate to at most `max` display columns without splitting a character.
pub fn truncate_display(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = visible_width(&ch.to_string());
        if used + w > max {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// Keep the last `max` display columns of `text` (character-aligned).
fn take_last_display(text: &str, max: usize) -> String {
    let mut rev = String::new();
    let mut used = 0usize;
    for ch in text.chars().rev() {
        let w = visible_width(&ch.to_string());
        if used + w > max {
            break;
        }
        rev.push(ch);
        used += w;
    }
    rev.chars().rev().collect()
}

/// Semantic color for a status segment; renderers map this to their theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegColor {
    Primary,
    Secondary,
    Warning,
    Success,
    Text,
    Muted,
}

/// A status-bar item that can render at three label widths and be dropped when
/// space runs out (unless priority keeps it).
#[derive(Debug, Clone)]
pub struct StatusSegment {
    pub full_label: &'static str,
    pub compact_label: &'static str,
    pub minimal_label: &'static str,
    pub value: String,
    pub color: SegColor,
    /// Lower = more important. Priority 0 is never hidden.
    pub priority: u8,
    pub allow_hide: bool,
}

impl StatusSegment {
    pub fn label(&self, form: LabelForm) -> &'static str {
        match form {
            LabelForm::Full => self.full_label,
            LabelForm::Compact => self.compact_label,
            LabelForm::Minimal => self.minimal_label,
        }
    }

    fn cell_width(&self, form: LabelForm) -> usize {
        let label = self.label(form);
        let label_w = if label.is_empty() {
            0
        } else {
            visible_width(label) + 1 // trailing space before value
        };
        label_w + visible_width(&self.value)
    }
}

/// One packed status cell (label already chosen; value ready to style).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedCell {
    pub label: &'static str,
    pub value: String,
    pub color: SegColor,
}

/// Pack status segments into at most `max_rows` rows of the given `width`,
/// separated by `sep_width` columns. Segments that do not fit and are hideable
/// are dropped; the count of dropped segments is returned so the caller can
/// surface a "+N" affordance. A row never exceeds `width`.
pub fn pack_status(
    segments: &[StatusSegment],
    width: usize,
    max_rows: usize,
    form: LabelForm,
    sep_width: usize,
) -> (Vec<Vec<PackedCell>>, usize) {
    let mut rows: Vec<Vec<PackedCell>> = Vec::new();
    let mut used = 0usize; // width used on the current row
    let mut hidden = 0usize;
    rows.push(Vec::new());

    for seg in segments {
        let cell_w = seg.cell_width(form);
        let current = rows.last_mut().expect("row exists");
        let sep = if current.is_empty() { 0 } else { sep_width };
        if used + sep + cell_w <= width {
            used += sep + cell_w;
            current.push(seg.to_cell(form));
            continue;
        }
        // Doesn't fit on this row. Try a new row if we have budget.
        if rows.len() < max_rows {
            rows.push(vec![seg.to_cell(form)]);
            used = cell_w.min(width);
            continue;
        }
        // No rows left: hide if allowed, otherwise force onto the last row.
        if seg.allow_hide && seg.priority > 0 {
            hidden += 1;
        } else {
            let current = rows.last_mut().expect("row exists");
            let sep = if current.is_empty() { 0 } else { sep_width };
            used += sep + cell_w;
            current.push(seg.to_cell(form));
        }
    }

    rows.retain(|r| !r.is_empty());
    if rows.is_empty() {
        rows.push(Vec::new());
    }
    (rows, hidden)
}

impl StatusSegment {
    fn to_cell(&self, form: LabelForm) -> PackedCell {
        PackedCell {
            label: self.label(form),
            value: self.value.clone(),
            color: self.color,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn width_class_boundaries() {
        assert_eq!(WidthClass::from_width(160), WidthClass::Wide);
        assert_eq!(WidthClass::from_width(120), WidthClass::Wide);
        assert_eq!(WidthClass::from_width(119), WidthClass::Desktop);
        assert_eq!(WidthClass::from_width(90), WidthClass::Desktop);
        assert_eq!(WidthClass::from_width(89), WidthClass::Compact);
        assert_eq!(WidthClass::from_width(65), WidthClass::Compact);
        assert_eq!(WidthClass::from_width(64), WidthClass::Narrow);
        assert_eq!(WidthClass::from_width(45), WidthClass::Narrow);
        assert_eq!(WidthClass::from_width(44), WidthClass::Mobile);
        assert_eq!(WidthClass::from_width(20), WidthClass::Mobile);
    }

    #[test]
    fn height_class_boundaries() {
        assert_eq!(HeightClass::from_height(40), HeightClass::Tall);
        assert_eq!(HeightClass::from_height(30), HeightClass::Tall);
        assert_eq!(HeightClass::from_height(29), HeightClass::Standard);
        assert_eq!(HeightClass::from_height(22), HeightClass::Standard);
        assert_eq!(HeightClass::from_height(21), HeightClass::Short);
        assert_eq!(HeightClass::from_height(16), HeightClass::Short);
        assert_eq!(HeightClass::from_height(15), HeightClass::VeryShort);
    }

    #[test]
    fn classify_desktop_shows_sidebar_single_row_chrome() {
        let rl = classify(rect(120, 35));
        assert_eq!(rl.width_class, WidthClass::Wide);
        assert!(rl.show_sidebar);
        assert_eq!(rl.header_rows, 1);
        assert_eq!(rl.status_rows, 1);
        assert!(!rl.too_small);
    }

    #[test]
    fn classify_mobile_stacks_and_hides_sidebar() {
        // Standard height keeps the full mobile stack (3 rows each).
        let rl = classify(rect(40, 24));
        assert_eq!(rl.width_class, WidthClass::Mobile);
        assert_eq!(rl.height_class, HeightClass::Standard);
        assert!(!rl.show_sidebar);
        assert_eq!(rl.header_rows, 3);
        assert_eq!(rl.status_rows, 3);
        assert_eq!(rl.input_max_rows, 3);
        assert!(rl.compact_labels);

        // Short mobile portrait clamps chrome rows to fit the height.
        let short = classify(rect(40, 20));
        assert_eq!(short.height_class, HeightClass::Short);
        assert_eq!(short.header_rows, 2);
        assert_eq!(short.status_rows, 2);
    }

    #[test]
    fn classify_very_short_collapses_chrome() {
        let rl = classify(rect(80, 12));
        assert_eq!(rl.header_rows, 1);
        assert_eq!(rl.status_rows, 1);
        assert_eq!(rl.input_max_rows, 1);
        assert!(!rl.show_sidebar);
    }

    #[test]
    fn classify_flags_too_small() {
        assert!(classify(rect(20, 6)).too_small);
        assert!(!classify(rect(24, 8)).too_small);
    }

    #[test]
    fn sandbox_short_maps_known_levels() {
        assert_eq!(sandbox_short("path-validation-only"), "path-only");
        assert_eq!(sandbox_short("restricted-local-process"), "restricted");
        assert_eq!(sandbox_short("disabled"), "off");
        assert_eq!(sandbox_short("mystery"), "mystery");
    }

    #[test]
    fn compact_path_project_only_and_tail() {
        assert_eq!(
            compact_path("/home/x/Airsec_Inc/SP_Product", 12, true),
            "SP_Product"
        );
        let tail = compact_path("/very/long/path/to/SP_Product", 14, false);
        assert!(tail.ends_with("SP_Product"));
        assert!(super::visible_width(&tail) <= 14);
        assert!(tail.starts_with('…'));
    }

    #[test]
    fn compact_path_keeps_wide_glyphs_whole() {
        // Each CJK glyph is width 2; truncation must not split one.
        let out = compact_path("项目目录名称", 5, true);
        assert!(super::visible_width(&out) <= 5);
    }

    #[test]
    fn pack_status_never_exceeds_width_and_hides_low_priority() {
        let segs = vec![
            StatusSegment {
                full_label: "STATUS",
                compact_label: "",
                minimal_label: "",
                value: "READY".into(),
                color: SegColor::Success,
                priority: 0,
                allow_hide: false,
            },
            StatusSegment {
                full_label: "MODEL",
                compact_label: "M",
                minimal_label: "M",
                value: "qwen3_4b".into(),
                color: SegColor::Primary,
                priority: 0,
                allow_hide: false,
            },
            StatusSegment {
                full_label: "VIEW",
                compact_label: "V",
                minimal_label: "V",
                value: "all·compact".into(),
                color: SegColor::Muted,
                priority: 3,
                allow_hide: true,
            },
        ];
        let (rows, hidden) = pack_status(&segs, 20, 1, LabelForm::Minimal, 3);
        for row in &rows {
            let w: usize = row
                .iter()
                .map(|c| {
                    let l = if c.label.is_empty() {
                        0
                    } else {
                        super::visible_width(c.label) + 1
                    };
                    l + super::visible_width(&c.value)
                })
                .sum::<usize>()
                + row.len().saturating_sub(1) * 3;
            assert!(w <= 20, "row width {w} exceeds 20");
        }
        // Priority-0 segments survive; the low-priority VIEW is hidden.
        assert!(hidden >= 1);
        let flat: Vec<&str> = rows.iter().flatten().map(|c| c.value.as_str()).collect();
        assert!(flat.contains(&"READY"));
        assert!(flat.contains(&"qwen3_4b"));
    }
}
