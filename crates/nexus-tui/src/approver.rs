//! Bridges the async agent loop's approval requests to the interactive UI.
//!
//! The loop calls `request_approval` on its own task; this handler forwards the
//! request to the render loop over a channel and blocks on a one-shot reply,
//! so the operator sees a modal and their keypress becomes the decision. If the
//! UI is gone, the request is denied — the safe default.

use nexus_agent::{
    ApprovalDecision, ApprovalHandler, PlanDecision, PlanReviewRequest, PlanReviewResponse,
};
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

/// A plan surfaced to the render loop for review.
///
/// Carried on its own channel rather than folded into [`ApprovalRequest`]: the
/// two ask different questions, offer different answers, and are rendered by
/// different surfaces. Sharing a channel would force one to pretend to be the
/// other.
pub struct PlanReview {
    pub request: PlanReviewRequest,
    pub reply: oneshot::Sender<PlanReviewResponse>,
}

pub struct TuiApprover {
    tx: mpsc::UnboundedSender<ApprovalRequest>,
    plan_tx: mpsc::UnboundedSender<PlanReview>,
}

impl TuiApprover {
    pub fn new(
        tx: mpsc::UnboundedSender<ApprovalRequest>,
        plan_tx: mpsc::UnboundedSender<PlanReview>,
    ) -> Self {
        Self { tx, plan_tx }
    }
}

#[async_trait::async_trait]
impl ApprovalHandler for TuiApprover {
    fn interactive(&self) -> bool {
        true
    }

    async fn review_plan(&self, request: &PlanReviewRequest) -> PlanReviewResponse {
        let (reply, rx) = oneshot::channel();
        let review = PlanReview {
            request: request.clone(),
            reply,
        };
        // If the UI is gone there is nobody to decide, and an unreviewed plan
        // must not execute — the same safe default the action approver takes.
        if self.plan_tx.send(review).is_err() {
            return PlanReviewResponse::to(request, PlanDecision::Decline);
        }
        rx.await
            .unwrap_or_else(|_| PlanReviewResponse::to(request, PlanDecision::Decline))
    }

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
