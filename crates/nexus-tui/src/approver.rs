//! Bridges the async agent loop's approval requests to the interactive UI.
//!
//! The loop calls `request_approval` on its own task; this handler forwards the
//! request to the render loop over a channel and blocks on a one-shot reply,
//! so the operator sees a modal and their keypress becomes the decision. If the
//! UI is gone, the request is denied — the safe default.

use nexus_agent::{ApprovalDecision, ApprovalHandler};
use nexus_policy::ActionRequest;
use tokio::sync::{mpsc, oneshot};

/// A request surfaced to the render loop.
pub struct ApprovalRequest {
    pub action: ActionRequest,
    pub arguments: serde_json::Value,
    pub reason: String,
    pub sandbox_active: bool,
    pub reply: oneshot::Sender<ApprovalDecision>,
}

pub struct TuiApprover {
    tx: mpsc::UnboundedSender<ApprovalRequest>,
}

impl TuiApprover {
    pub fn new(tx: mpsc::UnboundedSender<ApprovalRequest>) -> Self {
        Self { tx }
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for TuiApprover {
    async fn request_approval(
        &self,
        action: &ActionRequest,
        arguments: &serde_json::Value,
        reason: &str,
        sandbox_active: bool,
    ) -> ApprovalDecision {
        let (reply, rx) = oneshot::channel();
        let req = ApprovalRequest {
            action: action.clone(),
            arguments: arguments.clone(),
            reason: reason.to_string(),
            sandbox_active,
            reply,
        };
        if self.tx.send(req).is_err() {
            return ApprovalDecision::Deny;
        }
        rx.await.unwrap_or(ApprovalDecision::Deny)
    }
}
