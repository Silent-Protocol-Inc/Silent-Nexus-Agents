//! Input classification and slash-command parsing shared by the CLI's
//! interactive surfaces and the TUI input box.
//!
//! Three interaction modes: natural-language message, `/command`, and
//! `!command` shell shortcut (executed through the sandboxed terminal tool
//! path, never a raw shell). Slash detection is deliberately conservative:
//! only a single-line input whose first token looks like `/word` is treated
//! as a command; URLs, multi-line prose, and absolute paths (`/home/x`)
//! remain messages.

/// How one submitted input line should be handled.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// Nothing to do.
    Empty,
    /// A natural-language message for the agent.
    Message(String),
    /// A parsed `/command`.
    Slash(SlashCommand),
    /// A `!command` shell shortcut (body after the `!`).
    Shell(String),
}

/// A parsed slash command: `/name arg "quoted arg" --flag`.
#[derive(Debug, Clone, PartialEq)]
pub struct SlashCommand {
    /// Command word without the slash, lowercased.
    pub name: String,
    /// Tokenized arguments (quotes and escapes resolved).
    pub args: Vec<String>,
    /// The raw argument text exactly as typed (for free-text fast paths like
    /// `/goal Fix the parser`).
    pub rest: String,
}

/// A parse problem with an actionable message.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Classify one submitted input. Never panics on any input.
pub fn classify(raw: &str) -> Result<Input, ParseError> {
    let text = raw.trim();
    if text.is_empty() {
        return Ok(Input::Empty);
    }

    if let Some(body) = text.strip_prefix('!') {
        let body = body.trim();
        if body.is_empty() {
            return Err(ParseError {
                message: "`!` needs a command, e.g. `!git status`".into(),
            });
        }
        return Ok(Input::Shell(body.to_string()));
    }

    if looks_like_slash_command(text) {
        return parse_slash(text).map(Input::Slash);
    }

    Ok(Input::Message(text.to_string()))
}

/// True when the input should be parsed as a slash command:
/// single line, starts with `/` + letter, and the first token contains no
/// second `/` (so `/etc/hosts`, `//comment`, and URLs stay messages).
pub fn looks_like_slash_command(text: &str) -> bool {
    if text.contains('\n') {
        return false;
    }
    let Some(rest) = text.strip_prefix('/') else {
        return false;
    };
    let first = rest.split_whitespace().next().unwrap_or("");
    !first.is_empty()
        && first
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && !first.contains('/')
        && first
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn parse_slash(text: &str) -> Result<SlashCommand, ParseError> {
    let body = text.strip_prefix('/').unwrap_or(text);
    let (name, rest) = match body.split_once(char::is_whitespace) {
        Some((n, r)) => (n, r.trim()),
        None => (body, ""),
    };
    let args = tokenize(rest)?;
    Ok(SlashCommand {
        name: name.to_ascii_lowercase(),
        args,
        rest: rest.to_string(),
    })
}

/// Tokenize an argument string with shell-style quoting and escapes.
pub fn tokenize(text: &str) -> Result<Vec<String>, ParseError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    shell_words::split(text).map_err(|_| ParseError {
        message: "unbalanced quote — close the `\"`/`'` or escape it as `\\\"`".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn slash(raw: &str) -> SlashCommand {
        match classify(raw).expect("parse") {
            Input::Slash(s) => s,
            other => panic!("expected slash, got {other:?}"),
        }
    }

    #[test]
    fn empty_and_whitespace() {
        assert_eq!(classify("").expect("ok"), Input::Empty);
        assert_eq!(classify("   \t ").expect("ok"), Input::Empty);
    }

    #[test]
    fn plain_message() {
        assert_eq!(
            classify("Explain this repository").expect("ok"),
            Input::Message("Explain this repository".into())
        );
    }

    #[test]
    fn simple_slash() {
        let s = slash("/status");
        assert_eq!(s.name, "status");
        assert!(s.args.is_empty());
    }

    #[test]
    fn slash_with_args_and_case() {
        let s = slash("/Goal create Fix the slash command registration");
        assert_eq!(s.name, "goal");
        assert_eq!(s.args[0], "create");
        assert_eq!(s.rest, "create Fix the slash command registration");
    }

    #[test]
    fn quoted_arguments() {
        let s = slash(r#"/goal create "Fix the parser" --agent planner"#);
        assert_eq!(
            s.args,
            vec!["create", "Fix the parser", "--agent", "planner"]
        );
    }

    #[test]
    fn escaped_characters() {
        let s = slash(r#"/memory add project\ fact"#);
        assert_eq!(s.args, vec!["add", "project fact"]);
    }

    #[test]
    fn unbalanced_quote_is_helpful_error() {
        let err = classify(r#"/goal create "unterminated"#).expect_err("must fail");
        assert!(err.message.contains("unbalanced quote"));
    }

    #[test]
    fn urls_and_paths_are_messages() {
        for text in [
            "https://example.com/docs",
            "see /etc/hosts for details",
            "/home/user/project is the workspace",
            "//not a command",
            "/2fast (digit start)",
        ] {
            match classify(text).expect("ok") {
                Input::Message(_) => {}
                other => panic!("`{text}` should be a message, got {other:?}"),
            }
        }
    }

    #[test]
    fn multiline_input_is_a_message() {
        let text = "/status\nplease also check the logs";
        assert!(matches!(classify(text).expect("ok"), Input::Message(_)));
    }

    #[test]
    fn shell_shortcut() {
        assert_eq!(
            classify("!git status").expect("ok"),
            Input::Shell("git status".into())
        );
        assert!(classify("!").is_err());
    }

    #[test]
    fn never_crashes_on_garbage() {
        for text in ["/", "/\"", "/'''", "!\"", "/goal \\", "\u{0}/x", "/-flag"] {
            let _ = classify(text); // must not panic
        }
    }
}
