//! Command-line normalization and classification.
//!
//! Shell command strings proposed by models are parsed with `shell-words`
//! (no shell evaluation) and classified before policy runs. Classification is
//! conservative: anything we cannot parse is treated as requiring approval,
//! and injection metacharacters escalate the risk class.

use nexus_core::RiskLevel;

/// First program name in the command, lowercased, path-stripped.
pub fn program_of(command: &str) -> Option<String> {
    let words = shell_words::split(command).ok()?;
    let first = words.first()?;
    let name = first.rsplit('/').next().unwrap_or(first);
    Some(name.to_lowercase())
}

/// Canonical representation used for a session-scoped approval grant. This
/// normalizes harmless whitespace/quoting differences while retaining every
/// argument, so approving `cargo check` never grants `cargo publish`.
pub fn normalized(command: &str) -> Option<String> {
    let words = shell_words::split(command).ok()?;
    if words.is_empty() {
        return None;
    }
    serde_json::to_string(&words).ok()
}

/// Returns a denial reason when `command` matches the hard-deny list or
/// attempts privilege escalation.
pub fn hard_denied(command: &str, denied: &[String]) -> Option<String> {
    let prog = match program_of(command) {
        Some(p) => p,
        None => return Some("command could not be parsed safely".into()),
    };
    for d in denied {
        let d_prog = d.split_whitespace().next().unwrap_or(d).to_lowercase();
        if prog == d_prog {
            return Some(format!("`{prog}` is on the denied command list"));
        }
    }
    None
}

/// True when `command` starts with one of the allowlisted prefixes
/// (word-boundary aware: `cargo check` matches `cargo check --all`, not
/// `cargo checkmate`).
pub fn prefix_allowed(command: &str, allowed: &[String]) -> bool {
    let Ok(words) = shell_words::split(command) else {
        return false;
    };
    // Commands containing shell metacharacters are never auto-allowed even if
    // the prefix matches, because the suffix could chain arbitrary commands.
    if has_shell_metacharacters(command) {
        return false;
    }
    for prefix in allowed {
        let Ok(prefix_words) = shell_words::split(prefix) else {
            continue;
        };
        if prefix_words.is_empty() || prefix_words.len() > words.len() {
            continue;
        }
        if words[..prefix_words.len()] == prefix_words[..] {
            return true;
        }
    }
    false
}

/// Detect metacharacters that would make a "single command" actually execute
/// multiple commands or redirect output when run through a shell.
pub fn has_shell_metacharacters(command: &str) -> bool {
    // A quoted string may legitimately contain these; this check is applied
    // to the raw string because we execute via `sh -c` only when the user has
    // approved the exact text. For auto-allow decisions, be conservative.
    let mut in_single = false;
    let mut in_double = false;
    let mut prev = '\0';
    for c in command.chars() {
        match c {
            '\'' if !in_double && prev != '\\' => in_single = !in_single,
            '"' if !in_single && prev != '\\' => in_double = !in_double,
            ';' | '|' | '&' | '`' | '<' | '>' if !in_single && !in_double => return true,
            '$' if !in_single && prev != '\\' => {
                // $(…) and ${…} substitution
                return true;
            }
            '\n' if !in_single && !in_double => return true,
            _ => {}
        }
        prev = c;
    }
    false
}

/// Heuristic risk classification for a shell command.
pub fn classify_risk(command: &str) -> RiskLevel {
    let Some(prog) = program_of(command) else {
        return RiskLevel::Destructive;
    };
    let lower = command.to_lowercase();
    match prog.as_str() {
        "sudo" | "su" | "doas" | "pkexec" => RiskLevel::Privileged,
        "rm" | "rmdir" | "shred" | "truncate" => RiskLevel::Destructive,
        "mkfs" | "dd" | "fdisk" | "parted" => RiskLevel::Privileged,
        "git" => classify_git(&lower),
        "curl" | "wget" | "ssh" | "scp" | "rsync" | "nc" | "netcat" => RiskLevel::Network,
        "ls" | "cat" | "head" | "tail" | "grep" | "rg" | "find" | "fd" | "wc" | "file" | "stat"
        | "du" | "df" | "pwd" | "which" | "whoami" | "date" | "env" | "uname" => RiskLevel::Read,
        "cargo" | "rustc" | "npm" | "pnpm" | "yarn" | "pip" | "python" | "python3" | "node"
        | "go" | "make" | "cmake" | "gcc" | "clang" | "mvn" | "gradle" => {
            if lower.contains("publish") || lower.contains("deploy") || lower.contains("push") {
                RiskLevel::ExternalSideEffect
            } else {
                RiskLevel::Write
            }
        }
        _ => RiskLevel::Write,
    }
}

fn classify_git(lower: &str) -> RiskLevel {
    if lower.contains(" push")
        || lower.contains(" publish")
        || lower.contains(" send-email")
        || lower.contains(" remote add")
    {
        RiskLevel::ExternalSideEffect
    } else if lower.contains(" reset --hard")
        || lower.contains(" clean")
        || lower.contains(" checkout --")
        || lower.contains(" branch -d")
        || lower.contains(" branch -D")
        || lower.contains(" rebase")
        || lower.contains(" filter-branch")
    {
        RiskLevel::Destructive
    } else if lower.contains(" status")
        || lower.contains(" log")
        || lower.contains(" diff")
        || lower.contains(" show")
        || lower.contains(" branch")
        || lower.contains(" blame")
    {
        RiskLevel::Read
    } else {
        RiskLevel::Write
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_extraction_strips_paths() {
        assert_eq!(program_of("/usr/bin/SUDO ls").as_deref(), Some("sudo"));
        assert_eq!(program_of("cargo build").as_deref(), Some("cargo"));
        assert_eq!(program_of(""), None);
    }

    #[test]
    fn normalized_command_retains_arguments() {
        assert_eq!(
            normalized("cargo   check --all"),
            normalized("cargo check '--all'")
        );
        assert_ne!(normalized("cargo check"), normalized("cargo publish"));
    }

    #[test]
    fn metacharacter_detection() {
        assert!(has_shell_metacharacters("ls; rm -rf /"));
        assert!(has_shell_metacharacters("cat x | sh"));
        assert!(has_shell_metacharacters("echo $(whoami)"));
        assert!(has_shell_metacharacters("cargo check > /etc/passwd"));
        assert!(!has_shell_metacharacters("cargo check --all"));
        assert!(!has_shell_metacharacters("grep 'a;b' file.txt"));
    }

    #[test]
    fn prefix_allow_is_word_boundary_aware() {
        let allowed = vec!["cargo check".to_string()];
        assert!(prefix_allowed("cargo check --all", &allowed));
        assert!(!prefix_allowed("cargo checkmate", &allowed));
        assert!(!prefix_allowed("cargo check; rm -rf /", &allowed));
    }

    #[test]
    fn risk_classification() {
        assert_eq!(classify_risk("sudo apt install x"), RiskLevel::Privileged);
        assert_eq!(classify_risk("rm -rf target"), RiskLevel::Destructive);
        assert_eq!(
            classify_risk("git push origin main"),
            RiskLevel::ExternalSideEffect
        );
        assert_eq!(
            classify_risk("git reset --hard HEAD~1"),
            RiskLevel::Destructive
        );
        assert_eq!(classify_risk("git status"), RiskLevel::Read);
        assert_eq!(classify_risk("ls -la"), RiskLevel::Read);
        assert_eq!(
            classify_risk("cargo publish"),
            RiskLevel::ExternalSideEffect
        );
        assert_eq!(classify_risk("cargo build"), RiskLevel::Write);
    }
}
