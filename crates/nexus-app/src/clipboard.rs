//! Best-effort clipboard delivery for handoff summaries.

use base64::Engine;
use nexus_core::{NexusError, Result};
use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};

/// Copy text using OSC 52 for terminal clients, then native clipboard tools.
/// Callers retain a saved artifact as the final fallback.
pub fn copy(text: &str) -> Result<&'static str> {
    if std::io::stdout().is_terminal()
        && std::env::var("TERM")
            .map(|term| term != "dumb")
            .unwrap_or(true)
    {
        let mut stdout = std::io::stdout();
        stdout.write_all(osc52_sequence(text).as_bytes())?;
        stdout.flush()?;
        return Ok("OSC 52");
    }

    for (program, args) in native_candidates() {
        let Ok(mut child) = Command::new(program)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(stdin) = child.stdin.as_mut() {
            if stdin.write_all(text.as_bytes()).is_err() {
                let _ = child.kill();
                continue;
            }
        }
        if child.wait().map(|status| status.success()).unwrap_or(false) {
            return Ok(program);
        }
    }
    Err(NexusError::Other(
        "clipboard unavailable; use the saved summary artifact".into(),
    ))
}

fn osc52_sequence(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    format!("\x1b]52;c;{encoded}\x07")
}

fn native_candidates() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("pbcopy", &[]),
        ("clip.exe", &[]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_candidate_order_is_stable() {
        let names: Vec<_> = native_candidates().iter().map(|(name, _)| *name).collect();
        assert_eq!(names[0], "wl-copy");
        assert!(names.contains(&"pbcopy"));
    }

    #[test]
    fn osc52_encodes_without_embedding_plaintext() {
        let sequence = osc52_sequence("handoff");
        assert!(sequence.starts_with("\u{1b}]52;c;"));
        assert!(sequence.ends_with('\u{7}'));
        assert!(!sequence.contains("handoff"));
        assert!(sequence.contains("aGFuZG9mZg=="));
    }
}
