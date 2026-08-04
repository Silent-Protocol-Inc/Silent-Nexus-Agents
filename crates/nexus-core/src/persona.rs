//! The active behavioral persona: who the assistant *is* to the operator.
//!
//! A persona is not a label, a theme, or a style hint. Exactly one behavioral
//! persona reaches the model on every turn — the operator's selected persona,
//! or the built-in [`BUILTIN_NEXUS_PROMPT`] when none is selected. Never both,
//! never zero. [`BehavioralPersona::resolve`] is the only place that decision
//! is made, so "one active persona, never two" holds by construction rather
//! than by every call site remembering to check.
//!
//! What a persona controls stops at the model's *conduct*: identity, manner,
//! tone, format, creativity, relationship framing, content preferences. What it
//! can never touch is enforcement — permissions, sandbox scope, approvals,
//! credentials, tool availability, budgets. Those live in runtime code and are
//! decided before a single token is generated, which is why persona text asking
//! for them changes nothing.

use serde::{Deserialize, Serialize};

/// How persona text reaches a provider, strongest first.
///
/// This is capability reporting, not aspiration: an adapter that can only
/// prepend text to the first user turn must say [`InstructionChannel::PrefixFallback`]
/// so the inspector can tell the operator their persona carries less weight
/// there, instead of implying a system channel that does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionChannel {
    /// A real system role in the message list.
    SystemRole,
    /// A dedicated top-level instructions field outside the message list.
    InstructionsField,
    /// No system channel; instructions are prepended to conversational input.
    PrefixFallback,
    /// The adapter cannot carry application instructions at all.
    Unsupported,
}

impl InstructionChannel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SystemRole => "system role",
            Self::InstructionsField => "dedicated instructions field",
            Self::PrefixFallback => "prefix fallback",
            Self::Unsupported => "unsupported",
        }
    }

    /// Whether the persona is carried with true application-instruction
    /// authority. `false` means the model may weigh it no more heavily than
    /// conversation text, which the inspector must disclose.
    pub const fn is_application_authoritative(self) -> bool {
        matches!(self, Self::SystemRole | Self::InstructionsField)
    }

    /// Operator-facing note about what this channel costs. Empty when nothing
    /// needs disclosing.
    pub const fn limitation(self) -> &'static str {
        match self {
            Self::SystemRole | Self::InstructionsField => "",
            Self::PrefixFallback => {
                "this provider exposes no system channel; the persona is prepended to conversational input and may carry less weight than a system instruction"
            }
            Self::Unsupported => {
                "this provider accepts no application instructions; the persona cannot be delivered"
            }
        }
    }
}

/// Audience metadata. It labels a persona; it never edits one.
///
/// Deliberately inert: the profile is not consulted when the prompt is built,
/// so selecting `AdultsOnly` cannot add a hidden content rule, and selecting
/// `General` cannot quietly soften text the operator wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentProfile {
    #[default]
    General,
    Mature,
    AdultsOnly,
    Custom,
}

impl ContentProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Mature => "mature",
            Self::AdultsOnly => "adults_only",
            Self::Custom => "custom",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Mature => "Mature",
            Self::AdultsOnly => "Adults-only",
            Self::Custom => "Custom",
        }
    }

    /// Whether selecting this profile requires the one-time acknowledgment that
    /// the persona is for adult participants and adult fictional characters.
    pub const fn requires_acknowledgment(self) -> bool {
        matches!(self, Self::AdultsOnly)
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace(['-', ' '], "_")
            .as_str()
        {
            "general" => Some(Self::General),
            "mature" => Some(Self::Mature),
            "adults_only" | "adult" | "adults" => Some(Self::AdultsOnly),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// How a derived persona relates to its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InheritanceMode {
    /// Independent snapshot: the text was copied once and the base is gone.
    /// The default, because a copy cannot be broken later by editing something
    /// else.
    #[default]
    Snapshot,
    /// Live reference: the base's text is resolved at prompt time and this
    /// persona's text is appended.
    Extend,
}

impl InheritanceMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Extend => "extend",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "snapshot" | "copy" => Some(Self::Snapshot),
            "extend" | "inherit" => Some(Self::Extend),
            _ => None,
        }
    }
}

/// Whether a persona outlives the session that created it.
///
/// `SessionOnly` exists so a persona can be tried, or an isolated session can
/// run one, without a durable write nobody asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistencePolicy {
    #[default]
    Persistent,
    SessionOnly,
}

impl PersistencePolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Persistent => "persistent",
            Self::SessionOnly => "session_only",
        }
    }

    /// Whether definitions under this policy may be written to durable storage.
    pub const fn writes_durable_storage(self) -> bool {
        matches!(self, Self::Persistent)
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace(['-', ' '], "_")
            .as_str()
        {
            "persistent" | "saved" => Some(Self::Persistent),
            "session_only" | "session" | "temporary" => Some(Self::SessionOnly),
            _ => None,
        }
    }
}

/// The reserved identifier of the built-in persona. It is not stored as a row,
/// cannot be deleted, and cannot be taken by a user-created persona.
pub const BUILTIN_NEXUS_ID: &str = "persona_builtin_nexus";
pub const BUILTIN_NEXUS_NAME: &str = "Nexus";

/// The default behavioral identity.
///
/// Conduct only. It deliberately restates no safety rule: the safety layer is
/// pinned above every persona and enforced in code regardless, so repeating it
/// here would only mean a custom persona *appears* to drop protections it never
/// had the power to drop.
pub const BUILTIN_NEXUS_PROMPT: &str = "\
You are Nexus, the assistant Silent Nexus presents to the operator.

Identity:
- A capable engineering collaborator who owns the objective end to end: you plan, act, verify, and report.
- You speak for yourself, in your own voice, without narrating that you are an AI model or a persona.

Manner:
- Direct and concrete. Lead with the answer, then the reasoning that supports it.
- Plain language over ceremony. No filler openers, no flattery, no restating the request back.
- Say what you actually did and what you actually observed; never claim an outcome you have not seen.

Response style:
- Match length to the question — a sentence for a small one, structure for a large one.
- Prefer specifics: file paths, exact commands, real numbers, quoted output.
- When something is uncertain or unfinished, name it plainly rather than smoothing it over.
";

/// The built-in persona's operator-facing description.
pub const BUILTIN_NEXUS_DESCRIPTION: &str =
    "Silent Nexus's default voice: direct, concrete, evidence-first.";

/// Where a resolved persona came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonaOrigin {
    /// The operator selected a persona they (or an import) created.
    Custom,
    /// No persona is selected, so the built-in identity applies.
    BuiltIn,
}

impl PersonaOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::BuiltIn => "built_in",
        }
    }

    pub const fn is_custom(self) -> bool {
        matches!(self, Self::Custom)
    }
}

/// The one behavioral persona a turn runs under.
///
/// Constructed only by [`BehavioralPersona::resolve`] or
/// [`BehavioralPersona::built_in`], which is what makes the count exactly one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BehavioralPersona {
    pub id: String,
    pub name: String,
    /// Version of the persona definition this text came from. Recorded on the
    /// session and on delegated runs so a later edit is visible as a change
    /// rather than a silent substitution.
    pub revision: u32,
    pub origin: PersonaOrigin,
    pub content_profile: ContentProfile,
    /// The exact text sent to the model. Verbatim from storage: composed by
    /// declared inheritance, never summarized, filtered, or reworded.
    pub prompt: String,
}

impl BehavioralPersona {
    pub fn built_in() -> Self {
        Self {
            id: BUILTIN_NEXUS_ID.to_string(),
            name: BUILTIN_NEXUS_NAME.to_string(),
            revision: 1,
            origin: PersonaOrigin::BuiltIn,
            content_profile: ContentProfile::General,
            prompt: BUILTIN_NEXUS_PROMPT.to_string(),
        }
    }

    pub fn custom(
        id: impl Into<String>,
        name: impl Into<String>,
        revision: u32,
        content_profile: ContentProfile,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            revision,
            origin: PersonaOrigin::Custom,
            content_profile,
            prompt: prompt.into(),
        }
    }

    /// The single decision point. A custom persona with usable text replaces
    /// the built-in identity outright; anything else falls back to built-in.
    ///
    /// The empty-text guard matters: a persona whose prompt resolved to nothing
    /// would otherwise leave the turn with *zero* behavioral personas, which is
    /// as wrong as having two.
    pub fn resolve(custom: Option<Self>) -> Self {
        match custom {
            Some(persona) if !persona.prompt.trim().is_empty() => persona,
            _ => Self::built_in(),
        }
    }

    pub fn is_built_in(&self) -> bool {
        matches!(self.origin, PersonaOrigin::BuiltIn)
    }

    /// Label for the prompt section and the inspector, e.g.
    /// `active persona odysseus v3`.
    pub fn section_label(&self) -> String {
        format!("active persona {} v{}", self.name, self.revision)
    }

    /// Status-bar text at the widest layout.
    pub fn status_segment(&self) -> String {
        let marker = if self.content_profile.requires_acknowledgment() {
            " +"
        } else {
            ""
        };
        format!("PERSONA {}{marker}", self.name)
    }
}

/// Reserved identifiers a user-created persona may not claim.
pub fn is_reserved_persona_id(id: &str) -> bool {
    id.trim().eq_ignore_ascii_case(BUILTIN_NEXUS_ID)
}

/// Whether a name collides with the built-in persona.
pub fn is_reserved_persona_name(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case(BUILTIN_NEXUS_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_yields_exactly_one_persona() {
        let built_in = BehavioralPersona::resolve(None);
        assert!(built_in.is_built_in());
        assert_eq!(built_in.id, BUILTIN_NEXUS_ID);

        let custom = BehavioralPersona::custom(
            "persona_x",
            "odysseus",
            3,
            ContentProfile::Mature,
            "You are Odysseus.",
        );
        let resolved = BehavioralPersona::resolve(Some(custom.clone()));
        assert_eq!(resolved, custom);
        assert!(!resolved.is_built_in());
        // The custom text replaces the built-in identity rather than joining it.
        assert!(!resolved.prompt.contains("You are Nexus"));
    }

    #[test]
    fn an_empty_custom_persona_falls_back_instead_of_leaving_none() {
        // Zero behavioral personas is a defect, not a valid state: without this
        // the model would run with no identity at all.
        let blank = BehavioralPersona::custom("p", "blank", 1, ContentProfile::General, "   \n\t");
        assert!(BehavioralPersona::resolve(Some(blank)).is_built_in());
    }

    #[test]
    fn the_built_in_prompt_carries_conduct_and_not_enforcement() {
        let prompt = BUILTIN_NEXUS_PROMPT;
        assert!(prompt.contains("You are Nexus"));
        // Safety is pinned above every persona and enforced in code. Restating
        // it here would imply a custom persona could drop it by replacing this
        // text — it cannot, and the prompt must not suggest otherwise.
        for enforcement in [
            "workspace",
            "approval",
            "sandbox",
            "permission",
            "credential",
        ] {
            assert!(
                !prompt.to_ascii_lowercase().contains(enforcement),
                "the built-in persona must not restate enforcement: {enforcement}"
            );
        }
    }

    #[test]
    fn content_profile_is_metadata_and_never_rewrites_text() {
        let text = "Explicit consensual adult fiction. Swear freely.";
        for profile in [
            ContentProfile::General,
            ContentProfile::Mature,
            ContentProfile::AdultsOnly,
            ContentProfile::Custom,
        ] {
            let persona = BehavioralPersona::custom("p", "n", 1, profile, text);
            assert_eq!(
                persona.prompt,
                text,
                "{} mutated the prompt",
                profile.as_str()
            );
        }
        assert!(ContentProfile::AdultsOnly.requires_acknowledgment());
        assert!(!ContentProfile::Mature.requires_acknowledgment());
    }

    #[test]
    fn channels_report_their_real_authority() {
        assert!(InstructionChannel::SystemRole.is_application_authoritative());
        assert!(InstructionChannel::InstructionsField.is_application_authoritative());
        assert!(!InstructionChannel::PrefixFallback.is_application_authoritative());
        assert!(!InstructionChannel::Unsupported.is_application_authoritative());
        assert!(InstructionChannel::SystemRole.limitation().is_empty());
        assert!(!InstructionChannel::PrefixFallback.limitation().is_empty());
    }

    #[test]
    fn the_built_in_identity_is_reserved() {
        assert!(is_reserved_persona_id(BUILTIN_NEXUS_ID));
        assert!(is_reserved_persona_name("nexus"));
        assert!(!is_reserved_persona_name("odysseus"));
    }

    #[test]
    fn parsing_accepts_the_spellings_operators_type() {
        assert_eq!(
            ContentProfile::parse("adults-only"),
            Some(ContentProfile::AdultsOnly)
        );
        assert_eq!(
            ContentProfile::parse("Mature"),
            Some(ContentProfile::Mature)
        );
        assert_eq!(ContentProfile::parse("nonsense"), None);
        assert_eq!(
            InheritanceMode::parse("copy"),
            Some(InheritanceMode::Snapshot)
        );
        assert_eq!(
            PersistencePolicy::parse("session"),
            Some(PersistencePolicy::SessionOnly)
        );
        assert!(!PersistencePolicy::SessionOnly.writes_durable_storage());
    }
}
