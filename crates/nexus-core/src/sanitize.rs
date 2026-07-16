//! Terminal output sanitization.
//!
//! Tool output and model output may contain ANSI escape sequences designed to
//! rewrite the screen, spoof approval prompts, change the window title, or
//! exfiltrate data via OSC replies. Everything rendered into the TUI or
//! printed by the CLI passes through [`sanitize_terminal`] first.

/// Strip terminal control sequences, keeping printable text, newlines, and
/// tabs. CSI, OSC, DCS, APC, PM and single-char escapes are removed.
pub fn sanitize_terminal(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                // CSI: ESC [ ... final byte @-~
                Some('[') => {
                    chars.next();
                    for c2 in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c2) {
                            break;
                        }
                    }
                }
                // OSC / DCS / APC / PM: terminated by BEL or ST (ESC \)
                Some(']') | Some('P') | Some('_') | Some('^') => {
                    chars.next();
                    let mut prev_esc = false;
                    for c2 in chars.by_ref() {
                        if c2 == '\u{7}' || (prev_esc && c2 == '\\') {
                            break;
                        }
                        prev_esc = c2 == '\u{1b}';
                    }
                }
                // Two-char escapes (ESC c, ESC 7, …)
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            // Keep whitespace controls that are safe to render.
            '\n' | '\t' => out.push(c),
            '\r' => {} // drop CR to prevent line-overwrite spoofing
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Truncate to `max_bytes` on a char boundary, appending a marker when cut.
pub fn truncate_output(input: &str, max_bytes: usize) -> (String, bool) {
    if input.len() <= max_bytes {
        return (input.to_string(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    let mut s = input[..end].to_string();
    s.push_str("\n… [output truncated]");
    (s, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_sequences() {
        let s = "safe\u{1b}[2J\u{1b}[1;31mred\u{1b}[0m text";
        assert_eq!(sanitize_terminal(s), "safered text");
    }

    #[test]
    fn strips_osc_title_setting() {
        let s = "before\u{1b}]0;fake-title\u{7}after";
        assert_eq!(sanitize_terminal(s), "beforeafter");
    }

    #[test]
    fn strips_osc_with_st_terminator() {
        let s = "a\u{1b}]52;c;ZXhmaWw=\u{1b}\\b";
        assert_eq!(sanitize_terminal(s), "ab");
    }

    #[test]
    fn drops_carriage_return_spoofing() {
        let s = "$ rm -rf /\rls -la";
        assert_eq!(sanitize_terminal(s), "$ rm -rf /ls -la");
    }

    #[test]
    fn keeps_newlines_and_tabs() {
        let s = "line1\n\tindented";
        assert_eq!(sanitize_terminal(s), s);
    }

    #[test]
    fn truncates_on_char_boundary() {
        let (out, cut) = truncate_output("héllo wörld", 6);
        assert!(cut);
        assert!(out.starts_with("héllo") || out.starts_with("héll"));
    }
}
