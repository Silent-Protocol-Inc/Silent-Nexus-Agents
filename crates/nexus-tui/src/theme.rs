//! NEXUS semantic theme system.
//!
//! Every widget styles itself through named tokens (`primary`, `success`,
//! `warning`, …) — never raw colors — so themes and terminal capabilities
//! swap cleanly. Capability detection degrades: truecolor → 256 → 16 → none.
//! Status is never communicated by color alone; text markers accompany it.

use nexus_core::brand::{self, BasicColor, BrandColor};
use ratatui::style::{Color, Modifier, Style};
use std::io::IsTerminal;

pub use nexus_core::brand::ColorSupport;

/// Detect terminal color support from the environment.
pub fn detect_color_support(no_color: bool) -> ColorSupport {
    brand::detect_color_support(no_color, std::io::stdout().is_terminal())
}

/// One theme's palette, expressed as (truecolor, 256-index, 16-color) rows.
struct Palette {
    primary: (Color, u8, Color),
    secondary: (Color, u8, Color),
    success: (Color, u8, Color),
    warning: (Color, u8, Color),
    failure: (Color, u8, Color),
    muted: (Color, u8, Color),
    text: (Color, u8, Color),
    text_user: (Color, u8, Color),
    selection_bg: (Color, u8, Color),
}

const fn brand_row(color: BrandColor) -> (Color, u8, Color) {
    let ansi = match color.ansi16 {
        BasicColor::Cyan => Color::Cyan,
        BasicColor::Magenta => Color::Magenta,
        BasicColor::Green => Color::Green,
        BasicColor::Yellow => Color::Yellow,
        BasicColor::Red => Color::Red,
        BasicColor::DarkGray => Color::DarkGray,
        BasicColor::White => Color::White,
    };
    (
        Color::Rgb(color.rgb.0, color.rgb.1, color.rgb.2),
        color.ansi256,
        ansi,
    )
}

/// The default identity: neon cyan + ultraviolet on near-black.
const NEXUS_DARK: Palette = Palette {
    primary: brand_row(brand::BLUE),
    secondary: brand_row(brand::PINK),
    success: (Color::Rgb(94, 231, 145), 42, Color::Green),
    warning: (Color::Rgb(240, 200, 80), 220, Color::Yellow),
    failure: (Color::Rgb(255, 96, 110), 203, Color::Red),
    muted: brand_row(brand::SOFT_GRAY),
    text: brand_row(brand::TEXT),
    text_user: (Color::Rgb(170, 216, 255), 153, Color::Cyan),
    selection_bg: (Color::Rgb(34, 42, 66), 237, Color::Blue),
};

/// Ghost: colder, dimmer variant (steel + violet).
const GHOST: Palette = Palette {
    primary: (Color::Rgb(140, 190, 220), 110, Color::Cyan),
    secondary: (Color::Rgb(130, 120, 200), 104, Color::Magenta),
    success: (Color::Rgb(120, 200, 150), 72, Color::Green),
    warning: (Color::Rgb(210, 190, 120), 179, Color::Yellow),
    failure: (Color::Rgb(220, 120, 130), 174, Color::Red),
    muted: (Color::Rgb(95, 105, 125), 242, Color::DarkGray),
    text: (Color::Rgb(190, 196, 210), 251, Color::White),
    text_user: (Color::Rgb(160, 190, 230), 110, Color::Cyan),
    selection_bg: (Color::Rgb(40, 44, 58), 236, Color::Blue),
};

/// Cyberpunk: brighter cyan/ultraviolet with acid-green success.
const CYBERPUNK: Palette = Palette {
    primary: (Color::Rgb(0, 245, 255), 51, Color::Cyan),
    secondary: (Color::Rgb(190, 72, 255), 135, Color::Magenta),
    success: (Color::Rgb(173, 255, 47), 118, Color::Green),
    warning: (Color::Rgb(255, 191, 0), 214, Color::Yellow),
    failure: (Color::Rgb(255, 45, 85), 197, Color::Red),
    muted: (Color::Rgb(91, 112, 133), 242, Color::DarkGray),
    text: (Color::Rgb(224, 242, 255), 195, Color::White),
    text_user: (Color::Rgb(128, 255, 238), 122, Color::Cyan),
    selection_bg: (Color::Rgb(45, 21, 72), 53, Color::Magenta),
};

/// Edgerunner: electric yellow + ice blue with signal-red failure.
const EDGERUNNER: Palette = Palette {
    primary: (Color::Rgb(255, 229, 0), 226, Color::Yellow),
    secondary: (Color::Rgb(0, 204, 255), 45, Color::Cyan),
    success: (Color::Rgb(72, 255, 148), 84, Color::Green),
    warning: (Color::Rgb(255, 160, 0), 208, Color::Yellow),
    failure: (Color::Rgb(255, 42, 42), 196, Color::Red),
    muted: (Color::Rgb(100, 112, 126), 242, Color::DarkGray),
    text: (Color::Rgb(235, 240, 244), 255, Color::White),
    text_user: (Color::Rgb(140, 220, 255), 117, Color::Cyan),
    selection_bg: (Color::Rgb(55, 48, 4), 58, Color::Yellow),
};

const SYNTHWAVE: Palette = Palette {
    primary: (Color::Rgb(255, 92, 215), 206, Color::Magenta),
    secondary: (Color::Rgb(88, 122, 255), 69, Color::Blue),
    success: (Color::Rgb(72, 246, 196), 49, Color::Green),
    warning: (Color::Rgb(255, 199, 92), 221, Color::Yellow),
    failure: (Color::Rgb(255, 76, 126), 204, Color::Red),
    muted: (Color::Rgb(112, 92, 136), 60, Color::DarkGray),
    text: (Color::Rgb(245, 224, 255), 225, Color::White),
    text_user: (Color::Rgb(135, 211, 255), 117, Color::Cyan),
    selection_bg: (Color::Rgb(73, 27, 92), 54, Color::Magenta),
};

const NEON_NOIR: Palette = Palette {
    primary: (Color::Rgb(70, 220, 255), 45, Color::Cyan),
    secondary: (Color::Rgb(255, 72, 133), 204, Color::Magenta),
    success: (Color::Rgb(90, 220, 150), 78, Color::Green),
    warning: (Color::Rgb(224, 184, 88), 179, Color::Yellow),
    failure: (Color::Rgb(255, 73, 86), 203, Color::Red),
    muted: (Color::Rgb(78, 88, 105), 240, Color::DarkGray),
    text: (Color::Rgb(214, 220, 230), 253, Color::White),
    text_user: (Color::Rgb(149, 230, 255), 159, Color::Cyan),
    selection_bg: (Color::Rgb(30, 49, 63), 236, Color::Blue),
};

const ACID_RAIN: Palette = Palette {
    primary: (Color::Rgb(169, 255, 55), 118, Color::Green),
    secondary: (Color::Rgb(0, 216, 185), 43, Color::Cyan),
    success: (Color::Rgb(120, 255, 92), 119, Color::Green),
    warning: (Color::Rgb(238, 230, 76), 227, Color::Yellow),
    failure: (Color::Rgb(255, 79, 91), 203, Color::Red),
    muted: (Color::Rgb(79, 105, 83), 65, Color::DarkGray),
    text: (Color::Rgb(222, 243, 218), 194, Color::White),
    text_user: (Color::Rgb(183, 255, 150), 156, Color::Green),
    selection_bg: (Color::Rgb(36, 67, 35), 22, Color::Green),
};

const REDLINE: Palette = Palette {
    primary: (Color::Rgb(255, 62, 72), 203, Color::Red),
    secondary: (Color::Rgb(255, 142, 60), 209, Color::Yellow),
    success: (Color::Rgb(99, 222, 139), 78, Color::Green),
    warning: (Color::Rgb(255, 201, 79), 221, Color::Yellow),
    failure: (Color::Rgb(255, 35, 45), 196, Color::Red),
    muted: (Color::Rgb(112, 83, 86), 95, Color::DarkGray),
    text: (Color::Rgb(244, 226, 226), 224, Color::White),
    text_user: (Color::Rgb(255, 171, 154), 216, Color::Red),
    selection_bg: (Color::Rgb(76, 24, 29), 52, Color::Red),
};

const ICEWIRE: Palette = Palette {
    primary: (Color::Rgb(105, 224, 255), 81, Color::Cyan),
    secondary: (Color::Rgb(157, 173, 255), 147, Color::Blue),
    success: (Color::Rgb(91, 236, 201), 49, Color::Green),
    warning: (Color::Rgb(235, 213, 118), 186, Color::Yellow),
    failure: (Color::Rgb(255, 105, 132), 204, Color::Red),
    muted: (Color::Rgb(92, 115, 137), 67, Color::DarkGray),
    text: (Color::Rgb(225, 242, 250), 195, Color::White),
    text_user: (Color::Rgb(169, 225, 255), 153, Color::Cyan),
    selection_bg: (Color::Rgb(30, 58, 78), 24, Color::Blue),
};

const MATRIX: Palette = Palette {
    primary: (Color::Rgb(45, 255, 96), 83, Color::Green),
    secondary: (Color::Rgb(0, 190, 88), 35, Color::Green),
    success: (Color::Rgb(107, 255, 128), 120, Color::Green),
    warning: (Color::Rgb(211, 225, 80), 185, Color::Yellow),
    failure: (Color::Rgb(255, 82, 82), 203, Color::Red),
    muted: (Color::Rgb(60, 105, 72), 65, Color::DarkGray),
    text: (Color::Rgb(205, 240, 211), 194, Color::White),
    text_user: (Color::Rgb(129, 255, 151), 120, Color::Green),
    selection_bg: (Color::Rgb(24, 70, 35), 22, Color::Green),
};

const ULTRAVIOLET: Palette = Palette {
    primary: (Color::Rgb(177, 103, 255), 141, Color::Magenta),
    secondary: (Color::Rgb(98, 116, 255), 69, Color::Blue),
    success: (Color::Rgb(89, 235, 177), 79, Color::Green),
    warning: (Color::Rgb(249, 196, 87), 221, Color::Yellow),
    failure: (Color::Rgb(255, 83, 145), 205, Color::Red),
    muted: (Color::Rgb(102, 88, 128), 60, Color::DarkGray),
    text: (Color::Rgb(235, 226, 250), 189, Color::White),
    text_user: (Color::Rgb(194, 174, 255), 183, Color::Magenta),
    selection_bg: (Color::Rgb(57, 35, 87), 54, Color::Magenta),
};

const SOLAR_FLARE: Palette = Palette {
    primary: (Color::Rgb(255, 176, 51), 214, Color::Yellow),
    secondary: (Color::Rgb(255, 87, 51), 202, Color::Red),
    success: (Color::Rgb(120, 225, 116), 77, Color::Green),
    warning: (Color::Rgb(255, 222, 84), 227, Color::Yellow),
    failure: (Color::Rgb(255, 61, 51), 196, Color::Red),
    muted: (Color::Rgb(126, 96, 68), 95, Color::DarkGray),
    text: (Color::Rgb(250, 235, 211), 230, Color::White),
    text_user: (Color::Rgb(255, 202, 117), 222, Color::Yellow),
    selection_bg: (Color::Rgb(82, 47, 20), 52, Color::Red),
};

/// Semantic style provider handed to every renderer.
#[derive(Clone, Copy)]
pub struct Theme {
    support: ColorSupport,
    palette: &'static Palette,
    /// `mono`: keep structure, drop hue (accessibility / plain terminals).
    mono: bool,
}

impl Theme {
    /// A theme with no color at all.
    ///
    /// Used for measuring (row counts do not depend on styling) and for tests
    /// that assert on text, so an assertion can never accidentally pass or fail
    /// on an escape sequence.
    pub fn plain() -> Self {
        Self::new("nexus-dark", ColorSupport::None)
    }

    pub fn new(name: &str, support: ColorSupport) -> Self {
        let (palette, mono) = match name {
            "cyberpunk" => (&CYBERPUNK, false),
            "edgerunner" => (&EDGERUNNER, false),
            "ghost" => (&GHOST, false),
            "synthwave" => (&SYNTHWAVE, false),
            "neon-noir" => (&NEON_NOIR, false),
            "acid-rain" => (&ACID_RAIN, false),
            "redline" => (&REDLINE, false),
            "icewire" => (&ICEWIRE, false),
            "matrix" => (&MATRIX, false),
            "ultraviolet" => (&ULTRAVIOLET, false),
            "solar-flare" => (&SOLAR_FLARE, false),
            "mono" => (&NEXUS_DARK, true),
            _ => (&NEXUS_DARK, false),
        };
        Self {
            support,
            palette,
            mono,
        }
    }

    fn resolve(&self, row: (Color, u8, Color)) -> Option<Color> {
        if self.mono {
            return None;
        }
        match self.support {
            ColorSupport::TrueColor => Some(row.0),
            ColorSupport::Ansi256 => Some(Color::Indexed(row.1)),
            ColorSupport::Ansi16 => Some(row.2),
            ColorSupport::None => None,
        }
    }

    fn style(&self, row: (Color, u8, Color)) -> Style {
        match self.resolve(row) {
            Some(c) => Style::default().fg(c),
            None => Style::default(),
        }
    }

    // ------------------------------------------------------ semantic tokens

    /// Brand / interactive highlights (neon cyan).
    pub fn primary(&self) -> Style {
        self.style(self.palette.primary)
    }
    /// Secondary accent (ultraviolet).
    pub fn secondary(&self) -> Style {
        self.style(self.palette.secondary)
    }
    pub fn success(&self) -> Style {
        self.style(self.palette.success)
    }
    pub fn warning(&self) -> Style {
        self.style(self.palette.warning)
    }
    pub fn failure(&self) -> Style {
        self.style(self.palette.failure)
    }
    /// De-emphasized chrome: borders, hints, timestamps.
    pub fn muted(&self) -> Style {
        match self.resolve(self.palette.muted) {
            Some(c) => Style::default().fg(c),
            None => Style::default().add_modifier(Modifier::DIM),
        }
    }
    /// Body text.
    pub fn text(&self) -> Style {
        self.style(self.palette.text)
    }
    /// Operator-authored text.
    pub fn user(&self) -> Style {
        self.style(self.palette.text_user)
            .add_modifier(Modifier::BOLD)
    }
    /// Brand mark.
    pub fn brand(&self) -> Style {
        self.primary().add_modifier(Modifier::BOLD)
    }
    /// Selected row: background glow + bold (works color-free via REVERSED).
    pub fn selection(&self) -> Style {
        match self.resolve(self.palette.selection_bg) {
            Some(bg) => Style::default().bg(bg).add_modifier(Modifier::BOLD),
            None => Style::default().add_modifier(Modifier::REVERSED),
        }
    }
    /// Style for a report severity.
    pub fn sev(&self, sev: nexus_app::Sev) -> Style {
        match sev {
            nexus_app::Sev::Ok => self.success(),
            nexus_app::Sev::Warn => self.warning(),
            nexus_app::Sev::Err => self.failure(),
            nexus_app::Sev::Dim => self.muted(),
            nexus_app::Sev::Info => self.text(),
        }
    }
    /// Risk-level accent.
    pub fn risk(&self, level: &str) -> Style {
        match level {
            "read" | "network" => self.muted(),
            "write" => self.warning(),
            _ => self.failure().add_modifier(Modifier::BOLD),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_yields_plain_styles() {
        let t = Theme::new("nexus-dark", ColorSupport::None);
        assert_eq!(t.primary(), Style::default());
        // Muted still differentiates via DIM, selection via REVERSED.
        assert!(t.muted().add_modifier.contains(Modifier::DIM));
        assert!(t.selection().add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn capability_ladder() {
        let tc = Theme::new("nexus-dark", ColorSupport::TrueColor);
        assert!(matches!(tc.primary().fg, Some(Color::Rgb(..))));
        let a256 = Theme::new("nexus-dark", ColorSupport::Ansi256);
        assert!(matches!(a256.primary().fg, Some(Color::Indexed(_))));
        let a16 = Theme::new("nexus-dark", ColorSupport::Ansi16);
        assert_eq!(a16.primary().fg, Some(Color::Cyan));
    }

    #[test]
    fn mono_theme_drops_hue_keeps_structure() {
        let t = Theme::new("mono", ColorSupport::TrueColor);
        assert_eq!(t.primary().fg, None);
        assert!(t.user().add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn named_upgrade_themes_have_distinct_truecolor_accents() {
        let cyberpunk = Theme::new("cyberpunk", ColorSupport::TrueColor);
        let edgerunner = Theme::new("edgerunner", ColorSupport::TrueColor);
        assert_ne!(cyberpunk.primary().fg, edgerunner.primary().fg);
        assert!(matches!(cyberpunk.primary().fg, Some(Color::Rgb(..))));
        assert!(matches!(edgerunner.primary().fg, Some(Color::Rgb(..))));
    }

    #[test]
    fn cyberdeck_upgrade_theme_catalog_is_renderable() {
        let names = [
            "synthwave",
            "neon-noir",
            "acid-rain",
            "redline",
            "icewire",
            "matrix",
            "ultraviolet",
            "solar-flare",
        ];
        let accents: std::collections::BTreeSet<String> = names
            .iter()
            .map(|name| {
                let color = Theme::new(name, ColorSupport::TrueColor).primary().fg;
                assert!(matches!(color, Some(Color::Rgb(..))), "{name}");
                format!("{color:?}")
            })
            .collect();
        assert_eq!(accents.len(), names.len());
    }
}
