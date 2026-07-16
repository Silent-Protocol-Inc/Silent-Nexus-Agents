//! Structured command output rendered by both surfaces: the CLI prints it,
//! the TUI draws it into panels. Carrying structure (not pre-formatted text)
//! keeps color/width decisions with the renderer.

use nexus_core::brand::BrandVariant;

/// Severity for lines and fields; renderers map these to theme colors AND a
/// textual marker (status is never color-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sev {
    Info,
    Ok,
    Warn,
    Err,
    Dim,
}

#[derive(Debug, Clone)]
pub enum Item {
    Brand {
        variant: BrandVariant,
    },
    Header(String),
    Field {
        key: String,
        value: String,
        sev: Sev,
    },
    Line {
        text: String,
        sev: Sev,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

/// One command's output.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub title: Option<String>,
    pub items: Vec<Item>,
}

impl Report {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            items: Vec::new(),
        }
    }

    pub fn untitled() -> Self {
        Self::default()
    }

    pub fn header(mut self, text: impl Into<String>) -> Self {
        self.items.push(Item::Header(text.into()));
        self
    }

    pub fn brand(mut self, variant: BrandVariant) -> Self {
        self.items.push(Item::Brand { variant });
        self
    }

    pub fn field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.items.push(Item::Field {
            key: key.into(),
            value: value.into(),
            sev: Sev::Info,
        });
        self
    }

    pub fn field_sev(mut self, key: impl Into<String>, value: impl Into<String>, sev: Sev) -> Self {
        self.items.push(Item::Field {
            key: key.into(),
            value: value.into(),
            sev,
        });
        self
    }

    pub fn line(mut self, text: impl Into<String>) -> Self {
        self.items.push(Item::Line {
            text: text.into(),
            sev: Sev::Info,
        });
        self
    }

    pub fn line_sev(mut self, text: impl Into<String>, sev: Sev) -> Self {
        self.items.push(Item::Line {
            text: text.into(),
            sev,
        });
        self
    }

    pub fn ok(self, text: impl Into<String>) -> Self {
        self.line_sev(text, Sev::Ok)
    }

    pub fn warn(self, text: impl Into<String>) -> Self {
        self.line_sev(text, Sev::Warn)
    }

    pub fn error(self, text: impl Into<String>) -> Self {
        self.line_sev(text, Sev::Err)
    }

    pub fn table(mut self, headers: &[&str], rows: Vec<Vec<String>>) -> Self {
        self.items.push(Item::Table {
            headers: headers.iter().map(|h| h.to_string()).collect(),
            rows,
        });
        self
    }

    /// Flatten to plain text (used by tests and as a last-resort renderer).
    pub fn to_plain_text(&self) -> String {
        let mut out = String::new();
        if let Some(t) = &self.title {
            out.push_str(t);
            out.push('\n');
        }
        for item in &self.items {
            match item {
                Item::Brand { variant } => {
                    let lockup = nexus_core::brand::lockup(
                        *variant,
                        nexus_core::brand::BrandConstraints {
                            unicode: nexus_core::brand::unicode_supported(),
                            ..Default::default()
                        },
                    );
                    out.push_str(&lockup.plain_text());
                    out.push('\n');
                }
                Item::Header(h) => {
                    out.push_str("## ");
                    out.push_str(h);
                    out.push('\n');
                }
                Item::Field { key, value, .. } => {
                    out.push_str(&format!("{key:>16}  {value}\n"));
                }
                Item::Line { text, sev } => {
                    let mark = match sev {
                        Sev::Ok => "✓ ",
                        Sev::Warn => "! ",
                        Sev::Err => "✗ ",
                        _ => "",
                    };
                    out.push_str(mark);
                    out.push_str(text);
                    out.push('\n');
                }
                Item::Table { headers, rows } => {
                    out.push_str(&headers.join(" | "));
                    out.push('\n');
                    for r in rows {
                        out.push_str(&r.join(" | "));
                        out.push('\n');
                    }
                }
            }
        }
        out
    }
}
