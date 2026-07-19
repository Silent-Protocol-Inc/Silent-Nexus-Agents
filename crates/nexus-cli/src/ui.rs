//! Terminal rendering helpers for the `snx` CLI.
//!
//! All model- or web-derived text is passed through `nexus_core::sanitize`
//! before printing so control sequences can never reach the terminal. Color is
//! applied only to harness-authored labels, and is suppressed entirely in
//! no-color mode.

use nexus_core::brand::{
    self, BasicColor, BrandColor, BrandConstraints, BrandRole, BrandVariant, ColorSupport,
};
use nexus_core::sanitize::sanitize_terminal;
use std::io::IsTerminal;

/// ANSI palette implementing the canonical NEXUS identity, degrading to
/// plain text when color is disabled.
#[derive(Clone, Copy)]
pub struct Ui {
    support: ColorSupport,
}

const GREEN: BrandColor = BrandColor {
    rgb: (94, 231, 145),
    ansi256: 42,
    ansi16: BasicColor::Green,
};
const YELLOW: BrandColor = BrandColor {
    rgb: (240, 200, 80),
    ansi256: 220,
    ansi16: BasicColor::Yellow,
};
const RED: BrandColor = BrandColor {
    rgb: (255, 96, 110),
    ansi256: 203,
    ansi16: BasicColor::Red,
};
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

impl Ui {
    pub fn new(color: bool) -> Self {
        Self {
            support: brand::detect_color_support(!color, std::io::stdout().is_terminal()),
        }
    }

    #[cfg(test)]
    fn with_support(support: ColorSupport) -> Self {
        Self { support }
    }

    fn paint_code(&self, code: &str, text: &str) -> String {
        if self.support != ColorSupport::None {
            format!("{code}{text}{RESET}")
        } else {
            text.to_string()
        }
    }

    fn paint(&self, color: BrandColor, text: &str) -> String {
        let code = match self.support {
            ColorSupport::TrueColor => {
                format!("\x1b[38;2;{};{};{}m", color.rgb.0, color.rgb.1, color.rgb.2)
            }
            ColorSupport::Ansi256 => format!("\x1b[38;5;{}m", color.ansi256),
            ColorSupport::Ansi16 => match color.ansi16 {
                BasicColor::Cyan => "\x1b[36m".to_string(),
                BasicColor::Magenta => "\x1b[35m".to_string(),
                BasicColor::Green => "\x1b[32m".to_string(),
                BasicColor::Yellow => "\x1b[33m".to_string(),
                BasicColor::Red => "\x1b[31m".to_string(),
                BasicColor::DarkGray => "\x1b[90m".to_string(),
                BasicColor::White => "\x1b[37m".to_string(),
            },
            ColorSupport::None => return text.to_string(),
        };
        format!("{code}{text}{RESET}")
    }

    pub fn cyan(&self, t: &str) -> String {
        self.paint(brand::BLUE, t)
    }
    pub fn violet(&self, t: &str) -> String {
        self.paint(brand::PINK, t)
    }
    pub fn green(&self, t: &str) -> String {
        self.paint(GREEN, t)
    }
    pub fn yellow(&self, t: &str) -> String {
        self.paint(YELLOW, t)
    }
    pub fn red(&self, t: &str) -> String {
        self.paint(RED, t)
    }
    pub fn dim(&self, t: &str) -> String {
        self.paint_code(DIM, t)
    }
    pub fn bold(&self, t: &str) -> String {
        self.paint_code(BOLD, t)
    }

    fn brand_span(&self, role: BrandRole, text: &str, monochrome: bool) -> String {
        if monochrome {
            return match role {
                BrandRole::Icon | BrandRole::Wordmark => self.bold(text),
                BrandRole::Attribution | BrandRole::Tagline | BrandRole::Spacer => self.dim(text),
            };
        }
        match role {
            BrandRole::Icon => self.violet(text),
            BrandRole::Wordmark => self.bold(&self.cyan(text)),
            BrandRole::Attribution | BrandRole::Tagline => self.dim(text),
            BrandRole::Spacer => text.to_string(),
        }
    }

    pub fn render_brand(&self, variant: BrandVariant) {
        let interactive = std::io::stdout().is_terminal();
        let (width, height) = if interactive {
            crossterm::terminal::size().unwrap_or((80, 24))
        } else {
            (80, 24)
        };
        let lockup = brand::lockup(
            variant,
            BrandConstraints {
                width,
                height,
                unicode: brand::unicode_supported(),
            },
        );
        let outer_pad = if interactive {
            width.saturating_sub(lockup.width) / 2
        } else {
            2
        };
        println!();
        for line in &lockup.lines {
            let rendered = line
                .spans
                .iter()
                .map(|span| self.brand_span(span.role, &span.text, lockup.monochrome))
                .collect::<Vec<_>>()
                .concat();
            println!("{}{}", " ".repeat(outer_pad as usize), rendered);
        }
        println!();
    }

    /// The canonical compact NEXUS banner, shown on interactive entry.
    pub fn banner(&self) {
        self.render_brand(BrandVariant::Compact);
    }

    /// Print an info line with a colored key.
    pub fn field(&self, key: &str, value: &str) {
        println!(
            "  {}  {}",
            self.dim(&format!("{key:>14}")),
            self.safe(value)
        );
    }

    /// A section header.
    pub fn header(&self, title: &str) {
        println!("\n{}", self.bold(&self.cyan(title)));
    }

    pub fn ok(&self, msg: &str) {
        println!("{} {}", self.green("✓"), self.safe(msg));
    }

    pub fn warn(&self, msg: &str) {
        eprintln!("{} {}", self.yellow("!"), self.safe(msg));
    }

    /// Sanitize any untrusted string before display.
    pub fn safe(&self, text: &str) -> String {
        sanitize_terminal(text)
    }

    /// Color a risk label.
    pub fn risk(&self, level: &str) -> String {
        match level {
            "read" | "network" => self.dim(level),
            "write" => self.yellow(level),
            "destructive" | "privileged" | "external_side_effect" => self.red(level),
            other => other.to_string(),
        }
    }

    /// Render a structured [`nexus_app::Report`] (the shared command output).
    pub fn render_report(&self, report: &nexus_app::Report) {
        use nexus_app::{Item, Sev};
        if let Some(title) = &report.title {
            self.header(title);
        }
        for item in &report.items {
            match item {
                Item::Brand { variant } => self.render_brand(*variant),
                Item::Header(h) => self.header(h),
                Item::Field { key, value, sev } => {
                    let value = match sev {
                        Sev::Ok => self.green(&self.safe(value)),
                        Sev::Warn => self.yellow(&self.safe(value)),
                        Sev::Err => self.red(&self.safe(value)),
                        Sev::Dim => self.dim(&self.safe(value)),
                        Sev::Info => self.safe(value),
                    };
                    println!("  {}  {}", self.dim(&format!("{key:>16}")), value);
                }
                Item::Line { text, sev } => match sev {
                    Sev::Ok => println!("{} {}", self.green("✓"), self.safe(text)),
                    Sev::Warn => println!("{} {}", self.yellow("!"), self.safe(text)),
                    Sev::Err => println!("{} {}", self.red("✗"), self.safe(text)),
                    Sev::Dim => println!("  {}", self.dim(&self.safe(text))),
                    Sev::Info => println!("  {}", self.safe(text)),
                },
                Item::Table { headers, rows } => {
                    let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
                    self.table(&header_refs, rows);
                }
            }
        }
    }

    /// Render a simple two-or-more column table with aligned columns.
    pub fn table(&self, headers: &[&str], rows: &[Vec<String>]) {
        let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
        for row in rows {
            for (i, cell) in row.iter().enumerate() {
                if i < widths.len() {
                    widths[i] = widths[i].max(display_width(cell));
                }
            }
        }
        let head: Vec<String> = headers
            .iter()
            .enumerate()
            .map(|(i, h)| format!("{:<width$}", h, width = widths[i]))
            .collect();
        println!("  {}", self.bold(&self.cyan(&head.join("  "))));
        for row in rows {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let w = widths.get(i).copied().unwrap_or(0);
                    let pad = w.saturating_sub(display_width(c));
                    format!("{}{}", self.safe(c), " ".repeat(pad))
                })
                .collect();
            println!("  {}", cells.join("  "));
        }
    }
}

fn display_width(s: &str) -> usize {
    brand::visible_width(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_emits_no_escapes() {
        let ui = Ui::with_support(ColorSupport::None);
        let s = ui.cyan("hello");
        assert_eq!(s, "hello");
        assert!(!s.contains('\x1b'));
    }

    #[test]
    fn color_wraps_in_escapes() {
        let ui = Ui::with_support(ColorSupport::Ansi256);
        let s = ui.cyan("hi");
        assert!(s.contains('\x1b'));
        assert!(s.ends_with(RESET));
    }

    #[test]
    fn safe_strips_control_sequences() {
        let ui = Ui::with_support(ColorSupport::Ansi256);
        // An OSC injection attempt must not survive display.
        let out = ui.safe("ok\x1b]0;pwned\x07done");
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn risk_labels_map_to_colors_in_no_color_mode() {
        let ui = Ui::with_support(ColorSupport::None);
        assert_eq!(ui.risk("destructive"), "destructive");
    }

    #[test]
    fn brand_colors_follow_capability_ladder() {
        let truecolor = Ui::with_support(ColorSupport::TrueColor).cyan("NEXUS");
        assert!(truecolor.contains("38;2;125;211;252"));
        let indexed = Ui::with_support(ColorSupport::Ansi256).violet("NEXUS");
        assert!(indexed.contains("38;5;205"));
        let basic = Ui::with_support(ColorSupport::Ansi16).cyan("NEXUS");
        assert!(basic.contains("\x1b[36m"));
    }
}
