//! Minimal in-house HTML → readable text conversion.
//!
//! Deliberately dependency-free (see docs/dependencies.md): a full HTML5
//! parser is overkill for extracting readable text and would add a large
//! dependency tree. Limitations are documented: malformed markup degrades to
//! best-effort text, and scripts embedded via exotic encodings are simply
//! treated as text after tag stripping (the output is *data*, never executed
//! or rendered).

/// Extracted page content.
#[derive(Debug, Clone, Default)]
pub struct PageText {
    pub title: String,
    pub text: String,
}

/// Convert an HTML document to readable text: drops script/style/head
/// content, turns block boundaries into newlines, strips all tags, and
/// decodes common entities.
pub fn html_to_text(html: &str) -> PageText {
    let title = extract_between_ci(html, "<title", "</title>")
        .map(|t| {
            // <title …attrs> — cut everything through the first '>'
            let t = t.split_once('>').map(|(_, rest)| rest).unwrap_or(&t);
            decode_entities(t.trim())
        })
        .unwrap_or_default();

    let mut cleaned = remove_element_ci(html, "script");
    cleaned = remove_element_ci(&cleaned, "style");
    cleaned = remove_element_ci(&cleaned, "noscript");
    cleaned = remove_element_ci(&cleaned, "svg");
    cleaned = remove_element_ci(&cleaned, "head");
    // Strip HTML comments.
    cleaned = remove_between(&cleaned, "<!--", "-->");

    let block_tags = [
        "p",
        "div",
        "br",
        "li",
        "tr",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "section",
        "article",
        "header",
        "footer",
        "blockquote",
        "pre",
        "td",
    ];
    let mut out = String::with_capacity(cleaned.len() / 2);
    let mut chars = cleaned.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '<' {
            // Find tag name for block detection.
            let rest = &cleaned[i + 1..];
            let tag: String = rest
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_lowercase();
            if block_tags.contains(&tag.as_str()) && !out.ends_with('\n') {
                out.push('\n');
            }
            // Skip to the closing '>' (or end of input).
            for (_, c2) in chars.by_ref() {
                if c2 == '>' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }

    let decoded = decode_entities(&out);
    // Collapse whitespace: max one blank line, trim line ends.
    let mut lines: Vec<String> = Vec::new();
    let mut blank_run = 0usize;
    for line in decoded.lines() {
        let trimmed = collapse_spaces(line.trim());
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 && !lines.is_empty() {
                lines.push(String::new());
            }
        } else {
            blank_run = 0;
            lines.push(trimmed);
        }
    }
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    PageText {
        title,
        text: lines.join("\n"),
    }
}

fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out
}

/// Case-insensitive extraction of content between two markers.
fn extract_between_ci(haystack: &str, start: &str, end: &str) -> Option<String> {
    let lower = haystack.to_lowercase();
    let s = lower.find(&start.to_lowercase())?;
    let after = s + start.len();
    let e = lower[after..].find(&end.to_lowercase())? + after;
    Some(haystack[after..e].to_string())
}

/// Remove `<tag …>…</tag>` blocks case-insensitively (non-nested best effort).
fn remove_element_ci(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let lower = html.to_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(start_rel) = lower[pos..].find(&open) {
        let start = pos + start_rel;
        // Ensure it's a real tag boundary (`<script>` or `<script …`).
        let boundary_ok = lower[start + open.len()..]
            .chars()
            .next()
            .map(|c| c == '>' || c.is_whitespace() || c == '/')
            .unwrap_or(false);
        if !boundary_ok {
            out.push_str(&html[pos..start + open.len()]);
            pos = start + open.len();
            continue;
        }
        out.push_str(&html[pos..start]);
        match lower[start..].find(&close) {
            Some(end_rel) => {
                pos = start + end_rel + close.len();
            }
            None => {
                // Unclosed: drop the rest.
                pos = html.len();
            }
        }
    }
    out.push_str(&html[pos..]);
    out
}

fn remove_between(html: &str, start: &str, end: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;
    while let Some(s_rel) = html[pos..].find(start) {
        let s = pos + s_rel;
        out.push_str(&html[pos..s]);
        match html[s..].find(end) {
            Some(e_rel) => pos = s + e_rel + end.len(),
            None => {
                pos = html.len();
            }
        }
    }
    out.push_str(&html[pos..]);
    out
}

/// Decode the most common HTML entities plus numeric references.
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        let rest = &s[i + 1..];
        let semi = rest.find(';').filter(|&p| p <= 10);
        let Some(semi) = semi else {
            out.push('&');
            continue;
        };
        let entity = &rest[..semi];
        let decoded: Option<String> = match entity {
            "amp" => Some("&".into()),
            "lt" => Some("<".into()),
            "gt" => Some(">".into()),
            "quot" => Some("\"".into()),
            "apos" | "#39" | "#x27" => Some("'".into()),
            "nbsp" | "#160" => Some(" ".into()),
            "mdash" | "#8212" => Some("—".into()),
            "ndash" | "#8211" => Some("–".into()),
            "hellip" | "#8230" => Some("…".into()),
            "copy" => Some("©".into()),
            "rsquo" | "#8217" => Some("'".into()),
            "lsquo" | "#8216" => Some("'".into()),
            "rdquo" | "#8221" => Some("\u{201d}".into()),
            "ldquo" | "#8220" => Some("\u{201c}".into()),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32)
                .map(String::from),
            e if e.starts_with('#') => e[1..]
                .parse::<u32>()
                .ok()
                .and_then(char::from_u32)
                .map(String::from),
            _ => None,
        };
        match decoded {
            Some(d) => {
                out.push_str(&d);
                // Consume through the semicolon.
                for _ in 0..=semi {
                    chars.next();
                }
            }
            None => out.push('&'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_title_and_strips_scripts() {
        let html = r#"<html><head><title>My Page</title>
            <script>alert("evil")</script><style>.x{}</style></head>
            <body><h1>Header</h1><p>First &amp; second.</p>
            <script src="x.js"></script><div>More</div></body></html>"#;
        let page = html_to_text(html);
        assert_eq!(page.title, "My Page");
        assert!(page.text.contains("Header"));
        assert!(page.text.contains("First & second."));
        assert!(page.text.contains("More"));
        assert!(!page.text.contains("alert"));
        assert!(!page.text.contains(".x{}"));
    }

    #[test]
    fn numeric_entities_decode() {
        assert_eq!(decode_entities("a&#65;b&#x42;c"), "aAbBc");
        assert_eq!(
            decode_entities("5 &lt; 6 &amp;&amp; 7 &gt; 2"),
            "5 < 6 && 7 > 2"
        );
        // Unknown entity is left as-is.
        assert_eq!(decode_entities("&unknown;x"), "&unknown;x");
    }

    #[test]
    fn block_tags_become_newlines() {
        let page = html_to_text("<p>one</p><p>two</p><br>three");
        assert_eq!(page.text, "one\ntwo\nthree");
    }

    #[test]
    fn malformed_html_degrades_gracefully() {
        let page = html_to_text("<div><p>unclosed <b>bold text");
        assert!(page.text.contains("unclosed"));
        assert!(page.text.contains("bold text"));
    }
}
