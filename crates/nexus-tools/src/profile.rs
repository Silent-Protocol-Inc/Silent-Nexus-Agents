//! Profile cards (`profile.*`).
//!
//! The profile system was already complete — cards, facts with provenance and
//! sensitivity, identity-conflict resolution, a `/profile` surface to manage it
//! all. What was missing was any way for the agent to reach it. So an operator
//! who said "create a profile card and select it" was told, truthfully, that no
//! profile-management tool existed in the session, and the only honest advice
//! the agent could give was to go and do it by hand.
//!
//! This is the same shape of gap [`crate::memory`] closed: a capability the
//! product has, that the model has no verb for. The fix is the same too — real
//! typed tools over the canonical store, not prompt wording that claims success.
//!
//! **Reads, not workspace writes.** Every tool here is [`RiskLevel::Read`]. A
//! profile card is internal harness state in a separate store; it is not a file,
//! and classifying it as a write would deny it to exactly the read-only roles
//! that most need to know who they are talking to. What separates reading from
//! writing here is the `profile.write` capability (see
//! [`ToolMeta::required_capabilities`]), which the role resolution grants or
//! withholds — so a researcher can read the operator's language preference and
//! still cannot invent an identity fact from something it found on the web.
//!
//! **The port.** Cards live in the global store, while selecting one touches the
//! workspace store, the UI state, and the session row. All of that is owned by
//! the application control plane, which depends on this crate — so these tools
//! reach it through [`ProfilePort`] rather than reimplementing persistence they
//! would inevitably get subtly out of step.

use crate::{Tool, ToolCategory, ToolContext, ToolMeta, ToolOutput, ToolRegistry};
use nexus_core::{NexusError, Result, RiskLevel};
use nexus_policy::ActionRequest;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// The capability that separates reading the profile from changing it.
pub const WRITE_CAPABILITY: &str = "profile.write";

/// One profile card, as the model sees it.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    pub id: String,
    pub display_name: String,
    pub preferred_name: Option<String>,
    pub aliases: Vec<String>,
    pub active: bool,
    pub facts: Vec<FactView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FactView {
    pub id: String,
    pub key: String,
    pub value: String,
    /// `active` (in use) or `candidate` (waiting on a human).
    pub status: String,
    pub source: String,
    pub confidence: f64,
    pub sensitivity: String,
}

/// What a mutation actually did.
///
/// Reported rather than reduced to success/failure because the operator is told
/// which one happened, and "stored it", "replaced what was there", and "the card
/// already said that" are three different answers to "did you remember that".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Created,
    Updated,
    Unchanged,
    RequiresReview,
    Conflict,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Created => "created",
            Outcome::Updated => "updated",
            Outcome::Unchanged => "unchanged",
            Outcome::RequiresReview => "requires_review",
            Outcome::Conflict => "conflict",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Mutation {
    pub outcome: Outcome,
    pub profile_id: String,
    pub fact_id: Option<String>,
    pub active_profile_changed: bool,
    /// The sentence to report. Written here so the model repeats what happened
    /// rather than composing its own account of it.
    pub message: String,
}

/// The application's profile services, as this crate needs them.
///
/// Deliberately narrow: the tools ask for outcomes, never for a database
/// handle, so persistence, active-context switching, and UI-state sync stay in
/// the one place that already does them together and correctly.
pub trait ProfilePort: Send + Sync {
    fn active(&self) -> Result<Option<ProfileView>>;
    fn list(&self, include_archived: bool) -> Result<Vec<ProfileView>>;
    fn create(&self, display_name: &str, select: bool) -> Result<Mutation>;
    fn select(&self, profile_id: &str) -> Result<Mutation>;
    fn update(&self, profile_id: &str, preferred_name: Option<&str>) -> Result<Mutation>;
    fn add_fact(&self, key: &str, value: &str, sensitivity: &str) -> Result<Mutation>;
    fn remove_fact(&self, fact_id: &str) -> Result<Mutation>;
    fn merge(&self, from_profile_id: &str, into_profile_id: &str) -> Result<Mutation>;
    fn candidates(&self) -> Result<Vec<FactView>>;
    fn review_candidate(&self, fact_id: &str, approve: bool) -> Result<Mutation>;
}

/// The port, or a refusal that says why it is missing.
///
/// A tool that panics here would take the turn down; a tool that silently did
/// nothing would be worse. Contexts without a control plane are real — a bare
/// `ToolRegistry` in a test, for one — and the honest answer is that the service
/// is absent, not that the operator has no profile.
fn port(ctx: &ToolContext) -> Result<&Arc<dyn ProfilePort>> {
    ctx.profile.as_ref().ok_or_else(|| {
        NexusError::PolicyDenied(
            "profile management is not available in this context; nothing was stored".into(),
        )
    })
}

fn arg<'a>(args: &'a Value, name: &str, tool: &str) -> Result<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| NexusError::ToolInput {
            tool: tool.to_string(),
            message: format!("`{name}` is required"),
        })
}

fn report(mutation: &Mutation) -> ToolOutput {
    ToolOutput::text(mutation.message.clone()).with_metadata(json!({
        "outcome": mutation.outcome.as_str(),
        "profile_id": mutation.profile_id,
        "fact_id": mutation.fact_id,
        "active_profile_changed": mutation.active_profile_changed,
    }))
}

struct ProfileTool {
    meta: ToolMeta,
    action: Action,
}

#[derive(Clone, Copy)]
enum Action {
    GetActive,
    List,
    Create,
    Select,
    Update,
    AddFact,
    RemoveFact,
    Merge,
    GetCandidates,
    ReviewCandidate,
}

fn meta(
    name: &str,
    description: &str,
    input_schema: Value,
    writes: bool,
    side_effects: &str,
) -> ToolMeta {
    ToolMeta {
        name: name.into(),
        namespace: "profile".into(),
        description: description.into(),
        category: ToolCategory::Profile,
        input_schema,
        output_schema: json!({"type": "string"}),
        // Internal harness state in a separate store, not a workspace mutation.
        // What gates writing is the capability below, not the risk level — see
        // the module docs.
        risk: RiskLevel::Read,
        required_capabilities: if writes {
            vec![WRITE_CAPABILITY.to_string()]
        } else {
            vec![]
        },
        timeout_secs: 10,
        max_output_bytes: 8_000,
        deterministic: false,
        needs_network: false,
        needs_sandbox: false,
        side_effects: side_effects.into(),
    }
}

fn no_args() -> Value {
    json!({"type": "object", "properties": {}, "additionalProperties": false})
}

pub fn register(registry: &mut ToolRegistry) {
    let tools: Vec<(ToolMeta, Action)> = vec![
        (
            meta(
                "profile.get_active",
                "Read the operator's active profile card: who they are, what they want to be \
                 called, and the durable facts already recorded about them. Call it before \
                 assuming anything personal, and before recording a fact that may already be \
                 there.",
                no_args(),
                false,
                "reads profile state; changes nothing",
            ),
            Action::GetActive,
        ),
        (
            meta(
                "profile.list",
                "List the operator's profile cards and which one is active.",
                json!({
                    "type": "object",
                    "properties": {
                        "include_archived": {
                            "type": "boolean",
                            "description": "Include archived cards. Default false.",
                        },
                    },
                    "additionalProperties": false
                }),
                false,
                "reads profile state; changes nothing",
            ),
            Action::List,
        ),
        (
            meta(
                "profile.get_candidates",
                "List profile facts waiting on the operator's approval. These are recorded but \
                 not in use, so do not treat them as true.",
                no_args(),
                false,
                "reads profile state; changes nothing",
            ),
            Action::GetCandidates,
        ),
        (
            meta(
                "profile.create",
                "Create a profile card, optionally selecting it. Only when no suitable card \
                 exists — check profile.list first. Creating a second card for someone who \
                 already has one splits their history in two.",
                json!({
                    "type": "object",
                    "properties": {
                        "display_name": {
                            "type": "string",
                            "description": "What to call the card, normally the operator's name.",
                        },
                        "select": {
                            "type": "boolean",
                            "description": "Make it the active card. Default true — a card \
                                            nobody is using affects nothing.",
                        },
                    },
                    "required": ["display_name"],
                    "additionalProperties": false
                }),
                true,
                "creates one profile card and may change the active card",
            ),
            Action::Create,
        ),
        (
            meta(
                "profile.select",
                "Make an existing profile card the active one for this session.",
                json!({
                    "type": "object",
                    "properties": {"profile_id": {"type": "string"}},
                    "required": ["profile_id"],
                    "additionalProperties": false
                }),
                true,
                "changes which profile card is active",
            ),
            Action::Select,
        ),
        (
            meta(
                "profile.update",
                "Change what the operator is called on a card they already have.",
                json!({
                    "type": "object",
                    "properties": {
                        "profile_id": {"type": "string"},
                        "preferred_name": {
                            "type": "string",
                            "description": "What they want to be called.",
                        },
                    },
                    "required": ["profile_id", "preferred_name"],
                    "additionalProperties": false
                }),
                true,
                "updates one profile card",
            ),
            Action::Update,
        ),
        (
            meta(
                "profile.add_fact",
                "Record one durable fact about the operator on the active card — what they are \
                 called, what they do, their timezone, language, or a lasting working \
                 preference. Only facts they stated about themselves: never something inferred \
                 from the repository, from the web, or from how they seem. Never a credential; \
                 those are refused. The result says whether the fact is in use or waiting on \
                 their approval — report which, and do not claim it was stored if it was not.",
                json!({
                    "type": "object",
                    "properties": {
                        "key": {
                            "type": "string",
                            "description": "Canonical key, e.g. `identity.name`, \
                                            `identity.occupation`, `identity.timezone`, \
                                            `identity.language`, \
                                            `preferences.communication_style`, \
                                            `preferences.technical_stack`.",
                        },
                        "value": {"type": "string"},
                        "sensitivity": {
                            "type": "string",
                            "enum": ["normal", "sensitive"],
                            "description": "`sensitive` for health, religion, ethnicity, \
                                            politics, sexuality, finances, or precise location. \
                                            Sensitive facts are held for the operator's \
                                            approval rather than used.",
                        },
                    },
                    "required": ["key", "value"],
                    "additionalProperties": false
                }),
                true,
                "records one fact on the active profile card; changes nothing in the workspace",
            ),
            Action::AddFact,
        ),
        (
            meta(
                "profile.remove_fact",
                "Remove a fact from a profile card, when the operator says it is wrong or no \
                 longer true.",
                json!({
                    "type": "object",
                    "properties": {"fact_id": {"type": "string"}},
                    "required": ["fact_id"],
                    "additionalProperties": false
                }),
                true,
                "removes one fact from a profile card",
            ),
            Action::RemoveFact,
        ),
        (
            meta(
                "profile.merge",
                "Fold one profile card's facts into another and archive the source. For when \
                 the same person ended up with two cards.",
                json!({
                    "type": "object",
                    "properties": {
                        "from_profile_id": {"type": "string"},
                        "into_profile_id": {"type": "string"},
                    },
                    "required": ["from_profile_id", "into_profile_id"],
                    "additionalProperties": false
                }),
                true,
                "moves facts between profile cards and archives the source card",
            ),
            Action::Merge,
        ),
        (
            meta(
                "profile.review_candidate",
                "Approve or reject a pending profile fact, when the operator has said which.",
                json!({
                    "type": "object",
                    "properties": {
                        "fact_id": {"type": "string"},
                        "approve": {"type": "boolean"},
                    },
                    "required": ["fact_id", "approve"],
                    "additionalProperties": false
                }),
                true,
                "puts one pending fact into use, or rejects it",
            ),
            Action::ReviewCandidate,
        ),
    ];
    for (meta, action) in tools {
        registry.register(Arc::new(ProfileTool { meta, action }));
    }
}

#[async_trait::async_trait]
impl Tool for ProfileTool {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn action_request(&self, _args: &Value) -> Result<ActionRequest> {
        Ok(ActionRequest {
            tool: self.meta.name.clone(),
            risk: RiskLevel::Read,
            paths: vec![],
            formats: vec![],
            command: None,
            command_analysis: None,
            destination: None,
            summary: match self.action {
                Action::GetActive | Action::List | Action::GetCandidates => {
                    "read the operator's profile".to_string()
                }
                _ => "update the operator's profile".to_string(),
            },
        })
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutput> {
        let port = port(ctx)?;
        let name = self.meta.name.as_str();
        match self.action {
            Action::GetActive => {
                let active = port.active()?;
                Ok(match active {
                    Some(profile) => ToolOutput::text(
                        serde_json::to_string(&profile).unwrap_or_else(|_| "{}".into()),
                    ),
                    None => ToolOutput::text(
                        "no profile card is active; nothing is known about the operator yet",
                    ),
                })
            }
            Action::List => {
                let include_archived = args
                    .get("include_archived")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let profiles = port.list(include_archived)?;
                Ok(ToolOutput::text(
                    serde_json::to_string(&profiles).unwrap_or_else(|_| "[]".into()),
                ))
            }
            Action::GetCandidates => {
                let candidates = port.candidates()?;
                Ok(ToolOutput::text(if candidates.is_empty() {
                    "no profile facts are waiting for review".to_string()
                } else {
                    serde_json::to_string(&candidates).unwrap_or_else(|_| "[]".into())
                }))
            }
            Action::Create => {
                let display_name = arg(&args, "display_name", name)?;
                let select = args.get("select").and_then(Value::as_bool).unwrap_or(true);
                Ok(report(&port.create(display_name, select)?))
            }
            Action::Select => Ok(report(&port.select(arg(&args, "profile_id", name)?)?)),
            Action::Update => Ok(report(&port.update(
                arg(&args, "profile_id", name)?,
                Some(arg(&args, "preferred_name", name)?),
            )?)),
            Action::AddFact => {
                let sensitivity = args
                    .get("sensitivity")
                    .and_then(Value::as_str)
                    .unwrap_or("normal");
                Ok(report(&port.add_fact(
                    arg(&args, "key", name)?,
                    arg(&args, "value", name)?,
                    sensitivity,
                )?))
            }
            Action::RemoveFact => Ok(report(&port.remove_fact(arg(&args, "fact_id", name)?)?)),
            Action::Merge => Ok(report(&port.merge(
                arg(&args, "from_profile_id", name)?,
                arg(&args, "into_profile_id", name)?,
            )?)),
            Action::ReviewCandidate => {
                let approve = args
                    .get("approve")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| NexusError::ToolInput {
                        tool: name.to_string(),
                        message: "`approve` is required".into(),
                    })?;
                Ok(report(
                    &port.review_candidate(arg(&args, "fact_id", name)?, approve)?,
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        registry
    }

    /// The failure this whole module exists to fix: the category was reachable
    /// but resolved to nothing, so the agent correctly reported that no
    /// profile-management tool existed. Granting a category that carries no
    /// tool is indistinguishable, to the operator, from the feature not being
    /// built.
    #[test]
    fn the_profile_category_has_tools_to_carry_it() {
        let offered = registry().for_categories(&[ToolCategory::Profile]);
        assert!(
            offered.len() >= 10,
            "granting the profile category must resolve to callable tools, got {}",
            offered.len()
        );
        for tool in &offered {
            assert_eq!(tool.meta().category, ToolCategory::Profile);
        }
    }

    /// Reading who the operator is must not require permission to change it.
    #[test]
    fn reading_the_profile_needs_no_write_capability() {
        let registry = registry();
        for name in [
            "profile.get_active",
            "profile.list",
            "profile.get_candidates",
        ] {
            let tool = registry.get(name).expect("registered");
            assert!(
                tool.meta().required_capabilities.is_empty(),
                "`{name}` should be readable by any role",
            );
        }
    }

    /// Every mutation is gated on the capability, so a role that may read the
    /// profile still cannot invent facts about the person.
    #[test]
    fn every_mutation_is_gated_on_the_write_capability() {
        let registry = registry();
        for name in [
            "profile.create",
            "profile.select",
            "profile.update",
            "profile.add_fact",
            "profile.remove_fact",
            "profile.merge",
            "profile.review_candidate",
        ] {
            let tool = registry.get(name).expect("registered");
            assert_eq!(
                tool.meta().required_capabilities,
                vec![WRITE_CAPABILITY.to_string()],
                "`{name}` is ungated",
            );
        }
    }

    /// A profile card is internal state in a separate store, not a file. Rating
    /// it a write would deny it to the read-only roles that most need to know
    /// who they are talking to.
    #[test]
    fn touching_the_profile_is_never_a_workspace_write() {
        for tool in registry().all() {
            assert_eq!(tool.meta().risk, RiskLevel::Read, "{}", tool.meta().name);
            assert!(!tool.meta().needs_sandbox);
            assert!(!tool.meta().needs_network);
            assert_eq!(
                tool.action_request(&json!({})).expect("request").risk,
                RiskLevel::Read
            );
        }
    }

    /// The description is what the model reasons from, so it must not invite
    /// the one behaviour that would poison the card: writing down conclusions
    /// about the operator that the operator never stated.
    #[test]
    fn the_recording_tool_forbids_inferred_facts() {
        let description = registry()
            .get("profile.add_fact")
            .expect("registered")
            .meta()
            .description
            .clone();
        assert!(
            description.contains("never something inferred"),
            "{description}"
        );
        assert!(description.contains("Never a credential"), "{description}");
        assert!(
            description.contains("do not claim it was stored"),
            "{description}"
        );
    }

    /// Without a control plane there is no profile service. Saying so is the
    /// only honest answer — a silent success would be a lie about durable state.
    #[tokio::test]
    async fn a_context_without_the_service_refuses_instead_of_pretending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = crate::test_support::context(dir.path());
        let error = registry()
            .get("profile.add_fact")
            .expect("registered")
            .execute(&ctx, json!({"key": "identity.name", "value": "Sans"}))
            .await
            .expect_err("must refuse");
        assert!(error.to_string().contains("nothing was stored"), "{error}");
    }
}
