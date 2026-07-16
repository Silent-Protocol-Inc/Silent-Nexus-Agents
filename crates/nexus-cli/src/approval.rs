//! Approval handlers for the CLI.
//!
//! Interactive: prompts the operator on stdin, honestly displaying whether a
//! real sandbox is active. Non-interactive: denies every escalation by default
//! (`--yes` is a separate, explicit auto-approve handler) so an unattended run
//! can never silently perform a destructive or external action.

use crate::ui::Ui;
use nexus_agent::{ApprovalDecision, ApprovalHandler};
use nexus_policy::ActionRequest;
use std::io::{BufRead, Write};

/// Prompts the user on the terminal for each escalated action.
pub struct InteractiveApprover {
    ui: Ui,
}

impl InteractiveApprover {
    pub fn new(ui: Ui) -> Self {
        Self { ui }
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for InteractiveApprover {
    fn interactive(&self) -> bool {
        true
    }

    async fn request_approval(
        &self,
        action: &ActionRequest,
        arguments: &serde_json::Value,
        reason: &str,
        sandbox_active: bool,
    ) -> ApprovalDecision {
        let ui = self.ui;
        println!();
        println!("{}", ui.bold(&ui.yellow("─ approval required ─")));
        ui.field("tool", &action.tool);
        ui.field("risk", &ui.risk(&action.risk.to_string()));
        ui.field("summary", &action.summary);
        if let Some(cmd) = &action.command {
            ui.field("command", cmd);
        }
        if let Some(dest) = &action.destination {
            ui.field("destination", dest);
        }
        if !action.paths.is_empty() {
            ui.field("paths", &action.paths.join(", "));
        }
        ui.field("reason", reason);
        let iso = if sandbox_active {
            ui.green("active")
        } else {
            ui.red("NOT isolating this action")
        };
        ui.field("sandbox", &iso);

        // Read a decision. EOF / non-tty => deny (safe default).
        let persistent_allowed = sandbox_active && action.session_grant_allowed();
        let edit_allowed = !action
            .command_analysis
            .as_ref()
            .is_some_and(|analysis| analysis.one_time_only);
        print!(
            "  {} [o]nce{}{} / [N]o: ",
            ui.violet("decision"),
            if persistent_allowed {
                " / [s]ession"
            } else {
                ""
            },
            if edit_allowed { " / [e]dit safer" } else { "" }
        );
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let stdin = std::io::stdin();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            println!();
            return ApprovalDecision::Deny;
        }
        match line.trim().to_lowercase().as_str() {
            "y" | "yes" | "o" | "once" => ApprovalDecision::Approve,
            "s" | "session" if persistent_allowed => ApprovalDecision::ApproveForSession,
            "e" | "edit" | "alternative" if edit_allowed => {
                println!(
                    "  {}",
                    ui.dim("enter replacement tool arguments as one JSON object:")
                );
                println!("  current: {}", ui.safe(&arguments.to_string()));
                print!("  {} ", ui.violet("json>"));
                let _ = std::io::stdout().flush();
                let mut edited = String::new();
                if stdin.lock().read_line(&mut edited).unwrap_or(0) == 0 {
                    return ApprovalDecision::Deny;
                }
                match serde_json::from_str::<serde_json::Value>(edited.trim()) {
                    Ok(value) if value.is_object() => ApprovalDecision::ApproveEdited(value),
                    _ => {
                        println!("  {}", ui.red("invalid JSON object; denied"));
                        ApprovalDecision::Deny
                    }
                }
            }
            _ => ApprovalDecision::Deny,
        }
    }
}

/// Denies every escalation. Used for unattended runs unless `--yes` is passed.
pub struct AutoDenyApprover;

#[async_trait::async_trait]
impl ApprovalHandler for AutoDenyApprover {
    async fn request_approval(
        &self,
        _action: &ActionRequest,
        _arguments: &serde_json::Value,
        _reason: &str,
        _sandbox_active: bool,
    ) -> ApprovalDecision {
        ApprovalDecision::Deny
    }
}

/// Approves every escalation. Only installed when the operator passes `--yes`,
/// which is itself an explicit, audited authorization.
pub struct AutoApproveApprover;

#[async_trait::async_trait]
impl ApprovalHandler for AutoApproveApprover {
    async fn request_approval(
        &self,
        _action: &ActionRequest,
        _arguments: &serde_json::Value,
        _reason: &str,
        _sandbox_active: bool,
    ) -> ApprovalDecision {
        ApprovalDecision::Approve
    }
}
