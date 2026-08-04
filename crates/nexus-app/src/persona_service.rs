//! Persona management, shared by the TUI and the non-interactive CLI.
//!
//! Both surfaces call these functions; neither owns persistence. That is what
//! keeps `snx persona create` and PERSONA FORGE from drifting into two
//! different notions of what a persona is.
//!
//! Validation here is deliberately **technical only**. A persona is rejected
//! for being malformed, oversized, or for carrying terminal control sequences
//! or a credential — never for being profane, romantic, explicit, violent, or
//! otherwise unconventional. There is no keyword list in this file, and adding
//! one would defeat its purpose: a persona is the operator's own instruction to
//! their own model, and the provider remains free to refuse the *output*.

use crate::control_plane::PersonaMetadata;
use crate::App;
use nexus_core::persona::{
    BehavioralPersona, ContentProfile, InheritanceMode, InstructionChannel, PersistencePolicy,
    BUILTIN_NEXUS_DESCRIPTION, BUILTIN_NEXUS_ID, BUILTIN_NEXUS_NAME,
};
use nexus_core::{NexusError, Result};
use serde::{Deserialize, Serialize};

/// Portable persona document version. Bumped only when the shape changes in a
/// way an older reader cannot handle.
pub const PERSONA_EXPORT_SCHEMA: u32 = 1;

/// What a create or edit call was given.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PersonaSpec {
    pub name: String,
    pub description: String,
    pub instructions: String,
    /// `global` or `project`.
    pub scope: String,
    pub base_persona_id: Option<String>,
    pub inheritance_mode: InheritanceMode,
    pub content_profile: ContentProfile,
    pub category: String,
    pub tags: Vec<String>,
    pub compatibility_notes: String,
    pub recommended_providers: Vec<String>,
    pub recommended_models: Vec<String>,
    pub recommended_agents: Vec<String>,
    pub persistence_policy: PersistencePolicy,
    /// The operator confirmed an adults-only persona is for adult participants
    /// and adult fictional characters. Only this flag is kept — never identity
    /// data, and no document is ever requested.
    pub adult_acknowledged: bool,
    /// Sampling this persona wants while active. Unset fields leave the model's
    /// own configuration alone.
    pub sampling: nexus_core::persona::PersonaSampling,
    pub activate: bool,
}

impl PersonaSpec {
    pub fn new(name: impl Into<String>, instructions: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            instructions: instructions.into(),
            scope: "project".into(),
            ..Self::default()
        }
    }
}

/// A persona as the manager and `snx persona list` see it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub scope: String,
    pub revision: u32,
    pub content_profile: ContentProfile,
    pub category: String,
    pub tags: Vec<String>,
    pub base_persona_id: Option<String>,
    pub inheritance_mode: InheritanceMode,
    pub persistence_policy: PersistencePolicy,
    pub enabled: bool,
    pub selected: bool,
    pub built_in: bool,
}

/// The portable persona document. Behavioral definition only: no credentials,
/// no history, no profile facts, no runtime policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaDocument {
    pub schema_version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub system_prompt: String,
    #[serde(default)]
    pub content_profile: ContentProfile,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub inheritance_mode: InheritanceMode,
    #[serde(default)]
    pub revision: u32,
    #[serde(default)]
    pub compatibility_notes: String,
    #[serde(default)]
    pub recommended_providers: Vec<String>,
    #[serde(default)]
    pub recommended_models: Vec<String>,
    #[serde(default)]
    pub recommended_agents: Vec<String>,
}

/// Everything the effective-request inspector reports.
///
/// Every field here is either read from live state or computed by the same code
/// the turn uses. The previous version reported `behavioral_persona_count: 1`
/// and `duplicate_persona_sections: 0` as literals, describing the design
/// rather than the request — so it could not have detected a delivery problem
/// even in principle, which is exactly what an operator opens it to find.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveRequest {
    pub persona_name: String,
    pub persona_id: String,
    pub persona_revision: u32,
    pub content_profile: ContentProfile,
    pub custom_persona_active: bool,
    pub builtin_nexus_included: bool,
    pub behavioral_persona_count: usize,
    pub persona_is_system_instruction: bool,
    pub persona_is_user_message: bool,
    pub true_system_role_supported: bool,
    pub provider_restrictions_may_apply: bool,
    pub provider: String,
    pub model: String,
    pub instruction_channel: InstructionChannel,
    pub channel_limitation: String,
    pub persona_prompt: String,
    pub operational_contract: String,
    pub task_layer: String,
    pub duplicate_persona_sections: usize,
    /// The exact section body that would be sent — directive included, persona
    /// text verbatim. What you read here is what the provider receives.
    pub persona_section_body: String,
    /// Whether the section is prefixed with the sentence naming the persona as
    /// the identity to answer as.
    pub adoption_directive_present: bool,
    /// The persona opens the system block, before every other instruction.
    pub persona_emitted_first: bool,
    /// The temperature that would actually be sent — the persona's own, or the
    /// persona-layer default. Never null, because a turn carrying a persona
    /// always carries a temperature.
    pub persona_temperature: f32,
    /// Whether the persona named that temperature itself, or inherited the
    /// default. The number alone cannot distinguish the two.
    pub persona_temperature_is_default: bool,
    /// Output ceiling, or null when the parameter is omitted and the server
    /// picks its own.
    pub persona_max_output_tokens: Option<u32>,
    /// Which sections the *next* turn would carry, decided by the same function
    /// the loop calls.
    pub turn_shape: String,
    /// Plain sentence about what a hosted provider can still do to all of this.
    pub provider_caveat: String,
}

// ------------------------------------------------------------------ validation

/// The largest persona prompt accepted, in bytes. Sized for a long character
/// brief, not for a pasted repository.
pub const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_NAME_CHARS: usize = 64;

/// Reject only what cannot be stored or sent safely.
///
/// Tabs and newlines stay: a persona is prose and needs them. What goes is the
/// escape/CSI machinery a pasted file can smuggle in, which would otherwise
/// repaint the operator's terminal when the persona is displayed.
pub fn validate_prompt(instructions: &str) -> Result<()> {
    if instructions.trim().is_empty() {
        return Err(NexusError::Config(
            "persona instructions cannot be empty".into(),
        ));
    }
    if instructions.len() > MAX_PROMPT_BYTES {
        return Err(NexusError::Config(format!(
            "persona instructions are {} bytes; the maximum is {MAX_PROMPT_BYTES}",
            instructions.len()
        )));
    }
    if let Some(offset) = first_control_character(instructions) {
        return Err(NexusError::Config(format!(
            "persona instructions contain a terminal control character at byte {offset}; \
             remove escape sequences and retry"
        )));
    }
    Ok(())
}

/// Byte offset of the first disallowed control character, if any.
fn first_control_character(text: &str) -> Option<usize> {
    text.char_indices().find_map(|(index, ch)| {
        let allowed = ch == '\n' || ch == '\t' || ch == '\r';
        (!allowed && (ch.is_control() || ch == '\u{9b}')).then_some(index)
    })
}

pub fn validate_name(app: &App, name: &str, existing_id: Option<&str>) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(NexusError::Config("persona name cannot be empty".into()));
    }
    if trimmed.chars().count() > MAX_NAME_CHARS {
        return Err(NexusError::Config(format!(
            "persona name is longer than {MAX_NAME_CHARS} characters"
        )));
    }
    if nexus_core::persona::is_reserved_persona_name(trimmed) {
        return Err(NexusError::Config(format!(
            "`{BUILTIN_NEXUS_NAME}` is the built-in persona and cannot be redefined; \
             duplicate it under another name instead"
        )));
    }
    if let Ok(existing) = app.personas().get(trimmed) {
        if Some(existing.id.as_str()) != existing_id {
            return Err(NexusError::Config(format!(
                "a persona named `{trimmed}` already exists"
            )));
        }
    }
    Ok(())
}

fn validate_spec(app: &App, spec: &PersonaSpec, existing_id: Option<&str>) -> Result<()> {
    validate_name(app, &spec.name, existing_id)?;
    validate_prompt(&spec.instructions)?;
    // Caught here rather than at the provider: a persona asking for
    // temperature 5 should fail when it is saved, not halfway through the first
    // turn that uses it.
    spec.sampling.validate().map_err(NexusError::Config)?;
    if spec.content_profile.requires_acknowledgment() && !spec.adult_acknowledged {
        return Err(NexusError::Config(
            "an adults-only persona needs the one-time acknowledgment that it is intended \
             for adult participants and adult fictional characters"
                .into(),
        ));
    }
    if let Some(base) = spec.base_persona_id.as_deref() {
        if Some(base) == existing_id {
            return Err(NexusError::Config(
                "a persona cannot inherit from itself".into(),
            ));
        }
        app.personas()
            .get(base)
            .map_err(|_| NexusError::Config(format!("base persona `{base}` does not exist")))?;
    }
    Ok(())
}

fn metadata_from(spec: &PersonaSpec) -> PersonaMetadata {
    PersonaMetadata {
        content_profile: spec.content_profile,
        category: spec.category.trim().to_string(),
        // A snapshot keeps no live link: the text was already copied, and
        // recording a base would make a later edit of the source look like it
        // should flow through when it deliberately does not.
        base_persona_id: match spec.inheritance_mode {
            InheritanceMode::Extend => spec.base_persona_id.clone(),
            InheritanceMode::Snapshot => None,
        },
        inheritance_mode: spec.inheritance_mode,
        persistence_policy: spec.persistence_policy,
        enabled: true,
        compatibility_notes: spec.compatibility_notes.trim().to_string(),
        recommended_providers: spec.recommended_providers.clone(),
        recommended_models: spec.recommended_models.clone(),
        recommended_agents: spec.recommended_agents.clone(),
        adult_acknowledgment: spec.adult_acknowledged.then(nexus_core::now_rfc3339),
        sampling: spec.sampling,
    }
}

// --------------------------------------------------------------------- writes

/// Create a persona and, when asked, make it the active behavioral identity.
///
/// The text is stored exactly as given (trailing whitespace aside). Nothing
/// here inspects what it says.
pub fn create(app: &App, spec: &PersonaSpec) -> Result<PersonaSummary> {
    validate_spec(app, spec, None)?;
    if !spec.persistence_policy.writes_durable_storage() {
        // `SessionOnly` is a declared policy with no holding store behind it
        // yet — it exists so an isolated session can be built on it later
        // without reshaping the model. Saying "held for the session" here would
        // promise something no code does; refuse plainly instead.
        return Err(NexusError::Config(
            "session-only personas are not implemented yet: there is nowhere to hold one for \
             the session, and creating it would write it to storage anyway. Choose `persistent`."
                .into(),
        ));
    }
    let parent = match spec.inheritance_mode {
        InheritanceMode::Extend => spec.base_persona_id.as_deref(),
        InheritanceMode::Snapshot => None,
    };
    let id = app.personas().create(
        spec.name.trim(),
        scope_of(spec),
        parent,
        spec.description.trim(),
        &spec.instructions,
    )?;
    // Creation spans two stores, and the second one can still refuse — a
    // version conflict, a failed write. Without this, the refusal was reported
    // while the persona stayed in the first store, so `persona list` showed a
    // persona the operator had been told was not saved. Undo the first write
    // rather than leave the two disagreeing.
    let version = match app
        .harness()
        .sync_persona_with(&id, Some(metadata_from(spec)))
    {
        Ok(version) => version,
        Err(error) => {
            let _ = app.personas().delete(&id);
            return Err(error);
        }
    };
    if spec.activate {
        select(app, Some(&id))?;
    }
    summary_of(app, &id, &version)
}

/// Copy or extend an existing persona.
///
/// `Snapshot` — the default — takes the resolved text once and cuts the link,
/// so editing the copy cannot reach back into the source and deleting the
/// source cannot break the copy.
pub fn derive(app: &App, source_id: &str, mut spec: PersonaSpec) -> Result<PersonaSummary> {
    let source = app.personas().get(source_id)?;
    if spec.instructions.trim().is_empty() {
        spec.instructions = app.personas().resolved_instructions(&source.id)?;
    }
    if spec.description.trim().is_empty() {
        spec.description.clone_from(&source.description);
    }
    if spec.scope.trim().is_empty() {
        spec.scope.clone_from(&source.scope);
    }
    spec.base_persona_id = Some(source.id.clone());
    create(app, &spec)
}

/// Replace a persona's text and metadata, producing a new canonical revision.
pub fn edit(app: &App, id_or_name: &str, spec: &PersonaSpec) -> Result<PersonaSummary> {
    let existing = app.personas().get(id_or_name)?;
    validate_spec(app, spec, Some(existing.id.as_str()))?;
    let parent = match spec.inheritance_mode {
        InheritanceMode::Extend => spec.base_persona_id.as_deref(),
        InheritanceMode::Snapshot => None,
    };
    app.personas().update(
        &existing.id,
        spec.description.trim(),
        &spec.instructions,
        parent,
    )?;
    // As in `create`, the second store can refuse after the first has already
    // accepted. Here the previous text is known, so the undo restores it
    // instead of deleting a persona the operator did not ask to lose.
    let version = match app
        .harness()
        .sync_persona_with(&existing.id, Some(metadata_from(spec)))
    {
        Ok(version) => version,
        Err(error) => {
            let _ = app.personas().update(
                &existing.id,
                &existing.description,
                &existing.instructions,
                existing.parent_id.as_deref(),
            );
            return Err(error);
        }
    };
    // An edit to the persona that is currently active must reach the next turn,
    // not the next session: re-select it so the stored revision advances too.
    if app.read_ui_state(|state| state.selected_persona.as_deref() == Some(existing.id.as_str())) {
        select(app, Some(&existing.id))?;
    }
    summary_of(app, &existing.id, &version)
}

/// Select a persona, or clear the selection so the built-in identity returns.
///
/// Clearing writes `None` rather than remembering the last choice: the next
/// outbound request must contain Nexus and must not contain the persona the
/// operator just turned off.
pub fn select(app: &App, id_or_name: Option<&str>) -> Result<BehavioralPersona> {
    match id_or_name {
        Some(value) if !matches!(value, "none" | "off" | "clear" | BUILTIN_NEXUS_ID) => {
            let persona = app.personas().get(value)?;
            let version = app.harness().sync_persona(&persona.id)?;
            let id = persona.id.clone();
            app.update_ui_state(move |state| state.selected_persona = Some(id))?;
            let prompt = app.personas().resolved_instructions(&persona.id)?;
            Ok(version.behavioral(prompt))
        }
        _ => {
            app.update_ui_state(|state| state.selected_persona = None)?;
            Ok(BehavioralPersona::built_in())
        }
    }
}

/// Turn a persona off without deleting it. Resolution then falls back to the
/// built-in identity, exactly as if nothing were selected.
pub fn set_enabled(app: &App, id_or_name: &str, enabled: bool) -> Result<PersonaSummary> {
    let persona = app.personas().get(id_or_name)?;
    let current = latest_version(app, &persona.id)?;
    let mut metadata = current
        .as_ref()
        .map(PersonaMetadata::from)
        .unwrap_or_else(PersonaMetadata::new);
    metadata.enabled = enabled;
    let version = app
        .harness()
        .sync_persona_with(&persona.id, Some(metadata))?;
    if !enabled
        && app.read_ui_state(|state| state.selected_persona.as_deref() == Some(persona.id.as_str()))
    {
        select(app, None)?;
    }
    summary_of(app, &persona.id, &version)
}

pub fn delete(app: &App, id_or_name: &str) -> Result<String> {
    if nexus_core::persona::is_reserved_persona_id(id_or_name)
        || nexus_core::persona::is_reserved_persona_name(id_or_name)
    {
        return Err(NexusError::Config(format!(
            "the built-in `{BUILTIN_NEXUS_NAME}` persona cannot be deleted"
        )));
    }
    let persona = app.personas().get(id_or_name)?;
    if app.read_ui_state(|state| state.selected_persona.as_deref() == Some(persona.id.as_str())) {
        select(app, None)?;
    }
    app.personas().delete(&persona.id)?;
    Ok(persona.name)
}

// --------------------------------------------------------------- import/export

pub fn export(app: &App, id_or_name: &str) -> Result<PersonaDocument> {
    if is_built_in(id_or_name) {
        return Ok(PersonaDocument {
            schema_version: PERSONA_EXPORT_SCHEMA,
            name: BUILTIN_NEXUS_NAME.into(),
            description: BUILTIN_NEXUS_DESCRIPTION.into(),
            system_prompt: nexus_core::persona::BUILTIN_NEXUS_PROMPT.into(),
            content_profile: ContentProfile::General,
            category: "built-in".into(),
            tags: Vec::new(),
            inheritance_mode: InheritanceMode::Snapshot,
            revision: 1,
            compatibility_notes: String::new(),
            recommended_providers: Vec::new(),
            recommended_models: Vec::new(),
            recommended_agents: Vec::new(),
        });
    }
    let persona = app.personas().get(id_or_name)?;
    let prompt = app.personas().resolved_instructions(&persona.id)?;
    let version = latest_version(app, &persona.id)?;
    Ok(PersonaDocument {
        schema_version: PERSONA_EXPORT_SCHEMA,
        name: persona.name,
        description: persona.description,
        // Resolved, not raw: an export must stand on its own on a machine that
        // has never seen the base persona.
        system_prompt: prompt,
        content_profile: version
            .as_ref()
            .map(|v| v.content_profile)
            .unwrap_or_default(),
        category: version
            .as_ref()
            .map(|v| v.category.clone())
            .unwrap_or_default(),
        tags: version
            .as_ref()
            .map(|v| v.behavioral_tags.clone())
            .unwrap_or_default(),
        inheritance_mode: InheritanceMode::Snapshot,
        revision: version.as_ref().map(|v| v.version).unwrap_or(1),
        compatibility_notes: version
            .as_ref()
            .map(|v| v.compatibility_notes.clone())
            .unwrap_or_default(),
        recommended_providers: version
            .as_ref()
            .map(|v| v.recommended_providers.clone())
            .unwrap_or_default(),
        recommended_models: version
            .as_ref()
            .map(|v| v.recommended_models.clone())
            .unwrap_or_default(),
        recommended_agents: version
            .as_ref()
            .map(|v| v.recommended_agents.clone())
            .unwrap_or_default(),
    })
}

/// Import a persona document. Its text is stored verbatim; only structural
/// problems and the technical rules in [`validate_prompt`] can refuse it.
pub fn import(app: &App, raw: &str, activate: bool) -> Result<PersonaSummary> {
    let document: PersonaDocument = serde_json::from_str(raw)
        .map_err(|error| NexusError::Config(format!("persona import is not valid: {error}")))?;
    if document.schema_version == 0 || document.schema_version > PERSONA_EXPORT_SCHEMA {
        return Err(NexusError::Config(format!(
            "persona import declares schema version {}; this build reads up to {PERSONA_EXPORT_SCHEMA}",
            document.schema_version
        )));
    }
    let mut name = document.name.trim().to_string();
    if validate_name(app, &name, None).is_err() {
        name = unique_name(app, &name);
    }
    let spec = PersonaSpec {
        name,
        description: document.description,
        instructions: document.system_prompt,
        scope: "project".into(),
        base_persona_id: None,
        sampling: nexus_core::persona::PersonaSampling::default(),
        inheritance_mode: InheritanceMode::Snapshot,
        content_profile: document.content_profile,
        category: document.category,
        tags: document.tags,
        compatibility_notes: document.compatibility_notes,
        recommended_providers: document.recommended_providers,
        recommended_models: document.recommended_models,
        recommended_agents: document.recommended_agents,
        persistence_policy: PersistencePolicy::Persistent,
        // An imported adults-only persona arrives already classified; the
        // acknowledgment travels with the decision to import it.
        adult_acknowledged: document.content_profile.requires_acknowledgment(),
        activate,
    };
    create(app, &spec)
}

fn unique_name(app: &App, base: &str) -> String {
    let stem = if base.trim().is_empty() {
        "imported persona"
    } else {
        base.trim()
    };
    for suffix in 2..1000 {
        let candidate = format!("{stem} {suffix}");
        if validate_name(app, &candidate, None).is_ok() {
            return candidate;
        }
    }
    format!("{stem} {}", nexus_core::now_rfc3339())
}

// ---------------------------------------------------------------------- reads

pub fn list(app: &App) -> Result<Vec<PersonaSummary>> {
    let selected = app.read_ui_state(|state| state.selected_persona.clone());
    let versions = app.harness().workspace_persona_versions()?;
    let mut out = vec![PersonaSummary {
        id: BUILTIN_NEXUS_ID.into(),
        name: BUILTIN_NEXUS_NAME.into(),
        description: BUILTIN_NEXUS_DESCRIPTION.into(),
        scope: "built-in".into(),
        revision: 1,
        content_profile: ContentProfile::General,
        category: "built-in".into(),
        tags: Vec::new(),
        base_persona_id: None,
        inheritance_mode: InheritanceMode::Snapshot,
        persistence_policy: PersistencePolicy::Persistent,
        enabled: true,
        selected: selected.is_none(),
        built_in: true,
    }];
    for record in app.personas().list()? {
        let version = versions
            .iter()
            .filter(|version| version.persona_id == record.id)
            .max_by_key(|version| version.version);
        out.push(PersonaSummary {
            selected: selected.as_deref() == Some(record.id.as_str()),
            id: record.id,
            name: record.name,
            description: record.description,
            scope: record.scope,
            revision: version.map(|v| v.version).unwrap_or(1),
            content_profile: version.map(|v| v.content_profile).unwrap_or_default(),
            category: version.map(|v| v.category.clone()).unwrap_or_default(),
            tags: version
                .map(|v| v.behavioral_tags.clone())
                .unwrap_or_default(),
            base_persona_id: record.parent_id,
            inheritance_mode: version.map(|v| v.inheritance_mode).unwrap_or_default(),
            persistence_policy: version.map(|v| v.persistence_policy).unwrap_or_default(),
            enabled: version.map(|v| v.enabled).unwrap_or(true),
            built_in: false,
        });
    }
    Ok(out)
}

/// Personas eligible as a base for a new one. Empty means the base step is
/// hidden entirely rather than shown as a dead selector.
pub fn eligible_bases(app: &App) -> Result<Vec<PersonaSummary>> {
    Ok(list(app)?
        .into_iter()
        .filter(|persona| !persona.built_in)
        .collect())
}

/// The persona a turn started right now would run under.
pub fn active(app: &App) -> Result<BehavioralPersona> {
    let Some(selected) = app.read_ui_state(|state| state.selected_persona.clone()) else {
        return Ok(BehavioralPersona::built_in());
    };
    let Ok(record) = app.personas().get(&selected) else {
        // The selection points at something that no longer exists. Falling back
        // is right; refusing the turn is not.
        return Ok(BehavioralPersona::built_in());
    };
    let prompt = app.personas().resolved_instructions(&record.id)?;
    let version = latest_version(app, &record.id)?;
    if version.as_ref().is_some_and(|version| !version.enabled) {
        return Ok(BehavioralPersona::built_in());
    }
    Ok(BehavioralPersona::resolve(Some(match version {
        Some(version) => version.behavioral(prompt),
        None => {
            BehavioralPersona::custom(record.id, record.name, 1, ContentProfile::default(), prompt)
        }
    })))
}

pub fn resolved_prompt(app: &App, id_or_name: &str) -> Result<String> {
    if is_built_in(id_or_name) {
        return Ok(nexus_core::persona::BUILTIN_NEXUS_PROMPT.to_string());
    }
    let record = app.personas().get(id_or_name)?;
    app.personas().resolved_instructions(&record.id)
}

fn latest_version(
    app: &App,
    persona_id: &str,
) -> Result<Option<nexus_core::harness::PersonaVersion>> {
    Ok(app
        .harness()
        .workspace_persona_versions()?
        .into_iter()
        .filter(|version| version.persona_id == persona_id)
        .max_by_key(|version| version.version))
}

fn summary_of(
    app: &App,
    id: &str,
    version: &nexus_core::harness::PersonaVersion,
) -> Result<PersonaSummary> {
    let record = app.personas().get(id)?;
    let selected = app.read_ui_state(|state| state.selected_persona.clone());
    Ok(PersonaSummary {
        selected: selected.as_deref() == Some(record.id.as_str()),
        id: record.id,
        name: record.name,
        description: record.description,
        scope: record.scope,
        revision: version.version,
        content_profile: version.content_profile,
        category: version.category.clone(),
        tags: version.behavioral_tags.clone(),
        base_persona_id: record.parent_id,
        inheritance_mode: version.inheritance_mode,
        persistence_policy: version.persistence_policy,
        enabled: version.enabled,
        built_in: false,
    })
}

fn scope_of(spec: &PersonaSpec) -> &str {
    match spec.scope.trim() {
        "" => "project",
        other => other,
    }
}

pub fn is_built_in(id_or_name: &str) -> bool {
    nexus_core::persona::is_reserved_persona_id(id_or_name)
        || nexus_core::persona::is_reserved_persona_name(id_or_name)
}

/// The default `/persona test` question: it asks the model to state who it
/// thinks it is. Deliberately neutral — verifying that a persona took effect
/// must not require generating the persona's own subject matter.
pub const PERSONA_TEST_PROMPT: &str = "In four short lines, state: your name; your role; \
     your response style; and the two behavioral rules you are following most strictly.";

// ------------------------------------------------------------------------ test

/// Ask a model, through its real adapter, to state the persona it is running
/// under.
///
/// The request carries the active persona in the same position a turn would —
/// one system message, ahead of the question — so what comes back is evidence
/// about the delivered persona rather than about a prompt written for the test.
/// A provider refusal is reported as a refusal; it is never retried elsewhere.
pub async fn run_test(app: &App, model: &str, question: &str) -> Result<crate::report::Report> {
    use crate::report::{Report, Sev};
    use futures::StreamExt;

    let persona = active(app)?;
    let manager = nexus_models::ModelManager::from_config(&app.config)?;
    let provider = manager.get(model)?;
    let capabilities = provider.capabilities();
    let request = nexus_models::CompletionRequest {
        messages: vec![
            nexus_models::ChatMessage::system(persona.prompt.clone()),
            nexus_models::ChatMessage::user(question),
        ],
        max_tokens: Some(512),
        ..Default::default()
    };
    let started = std::time::Instant::now();
    let mut stream = provider.stream(request).await?;
    let mut answer = String::new();
    while let Some(event) = stream.next().await {
        match event? {
            nexus_models::StreamEvent::TextDelta(text) => answer.push_str(&text),
            nexus_models::StreamEvent::Done { .. } => break,
            _ => {}
        }
    }
    let elapsed = started.elapsed().as_millis();
    let mut report = Report::new(format!("persona test — {}", persona.name))
        .field("model", model)
        .field("provider", &capabilities.provider_kind)
        .field(
            "delivered through",
            capabilities.instruction_channel.as_str(),
        )
        .field("elapsed", format!("{elapsed} ms"));
    if !capabilities.instruction_channel.limitation().is_empty() {
        report = report.warn(capabilities.instruction_channel.limitation());
    }
    if answer.trim().is_empty() {
        report = report.field_sev(
            "answer",
            "the model returned no text — this is the provider's response, not a persona failure",
            Sev::Warn,
        );
    }
    Ok(report
        .header("question")
        .line(question)
        .header("answer")
        .line(answer.trim()))
}

// ------------------------------------------------------------------- inspector

/// Report exactly how the next request is composed.
///
/// The persona and the operational contract are read from the same functions
/// the turn itself uses — [`active`] and [`nexus_agent::AgentRole::charter`] —
/// so the inspector cannot drift into describing a request that is not the one
/// being sent.
pub fn effective_request(app: &App) -> Result<EffectiveRequest> {
    let persona = active(app)?;
    let role = active_role(app);
    let model = app.any_model_name();
    let (provider_kind, channel) = provider_channel(app, &model);

    // Built with the same calls the turn makes, so what is reported is what
    // would be sent rather than a second description of it.
    let persona_config = &app.config.persona;
    let section_body = if persona_config.adoption_directive {
        persona.section_body()
    } else {
        persona.prompt.clone()
    };
    let adoption_directive_present = section_body.starts_with("Your name is ");
    let persona_sampling = persona.sampling;
    // Counted from the section that would actually be emitted. One persona is a
    // property of `BehavioralPersona::resolve` returning a single value — but
    // counting the sections is what would notice if a second one ever appeared,
    // and asserting it costs nothing.
    let behavioral_persona_count = usize::from(!section_body.trim().is_empty());
    let duplicate_persona_sections = section_body
        .matches("Your name is ")
        .count()
        .saturating_sub(1);
    // The shape the next turn would take, from the loop's own decision
    // function. A bare identity question is the case the operator hit.
    let shape = nexus_agent::prompt_shape::PromptShape::decide(
        "who are you?",
        false,
        false,
        &nexus_core::orchestration::WorkBreakdown::generate(
            "who are you?",
            nexus_core::orchestration::WorkEstimate::from_objective("who are you?"),
        ),
        persona_config,
    );
    let contract = operational_contract(role);
    Ok(EffectiveRequest {
        persona_name: persona.name.clone(),
        persona_id: persona.id.clone(),
        persona_revision: persona.revision,
        content_profile: persona.content_profile,
        custom_persona_active: !persona.is_built_in(),
        builtin_nexus_included: persona.is_built_in(),
        behavioral_persona_count,
        persona_is_system_instruction: channel.is_application_authoritative(),
        // Structural, not observational: the compiler emits every context
        // section with the system role and appends conversation history after
        // it. There is no path that turns a persona into user content.
        persona_is_user_message: false,
        true_system_role_supported: matches!(channel, InstructionChannel::SystemRole),
        provider_restrictions_may_apply: true,
        provider: provider_kind,
        model,
        instruction_channel: channel,
        channel_limitation: channel.limitation().to_string(),
        persona_prompt: persona.prompt,
        operational_contract: contract,
        task_layer: TASK_LAYER_SUMMARY.into(),
        duplicate_persona_sections,
        persona_section_body: section_body,
        adoption_directive_present,
        // Not a runtime measurement: the section is constructed with
        // `WirePosition::First` in one place, and `persona_requests.rs` asserts
        // on the recorded outbound request that it really does open the system
        // block. Reported here so the operator can see the intent alongside the
        // test that holds it.
        persona_emitted_first: true,
        persona_temperature: persona_sampling.effective_temperature(),
        persona_temperature_is_default: persona_sampling.temperature.is_none(),
        persona_max_output_tokens: persona_sampling.max_output_tokens,
        turn_shape: shape.describe().to_string(),
        provider_caveat: PROVIDER_CAVEAT.into(),
    })
}

/// What Silent Nexus cannot do anything about, said plainly.
///
/// A hosted backend applies its own identity and content policy server-side,
/// above whatever the application sends. Delivering the persona better does not
/// change that, and claiming otherwise would be the kind of promise this
/// inspector exists to avoid.
const PROVIDER_CAVEAT: &str =
    "A hosted provider applies its own instructions and content policy on the server, above \
     anything sent from here. It may answer in its own voice or decline regardless of how the \
     persona is delivered. Silent Nexus does not rewrite the persona to pre-empt that and does \
     not reroute to another provider around it; running a local model is the way to remove the \
     other party from the decision.";

/// What the task layer carries. Named rather than dumped: its contents are the
/// live request, goal, plan, and retrieved evidence for the turn in flight,
/// which do not exist until a turn starts.
const TASK_LAYER_SUMMARY: &str =
    "current user request · active goal · approved plan · current task · acceptance criteria \
     · retrieved context · verified evidence · runtime state";

fn active_role(app: &App) -> nexus_agent::AgentRole {
    app.read_ui_state(|state| state.active_agent.clone())
        .as_deref()
        .and_then(nexus_agent::AgentRole::parse)
        .unwrap_or(nexus_agent::AgentRole::Nexus)
}

fn operational_contract(role: nexus_agent::AgentRole) -> String {
    let charter = role.charter();
    let contract = role.output_contract();
    if charter.is_empty() {
        contract.to_string()
    } else {
        format!("{contract}\n{charter}")
    }
}

/// The instruction channel for a configured model, and its provider kind.
///
/// Falls back to [`InstructionChannel::SystemRole`] only when the model cannot
/// be constructed at all — an unreachable endpoint still reports its adapter's
/// real channel, because capabilities are static metadata, not a probe.
fn provider_channel(app: &App, model: &str) -> (String, InstructionChannel) {
    let Ok(manager) = nexus_models::ModelManager::from_config(&app.config) else {
        return ("unknown".into(), InstructionChannel::SystemRole);
    };
    match manager.get(model) {
        Ok(provider) => {
            let capabilities = provider.capabilities();
            (
                capabilities.provider_kind.clone(),
                capabilities.instruction_channel,
            )
        }
        Err(_) => ("unknown".into(), InstructionChannel::SystemRole),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn technical_validation_rejects_only_technical_problems() {
        assert!(validate_prompt("").is_err());
        assert!(validate_prompt("   ").is_err());
        assert!(validate_prompt(&"x".repeat(MAX_PROMPT_BYTES + 1)).is_err());
        assert!(validate_prompt("clean text\nwith\tlines\r\n").is_ok());
        assert!(validate_prompt("escape \u{1b}[31m here").is_err());
        assert!(validate_prompt("csi \u{9b}0m here").is_err());
        assert!(validate_prompt("null \u{0} byte").is_err());
    }

    #[test]
    fn mature_and_explicit_text_is_technically_valid() {
        // The point of the persona rules: SNX stores what the operator wrote.
        // These strings exist so a future keyword filter fails this test.
        for text in [
            "You are explicit, profane, and sexually forward with consenting adults.",
            "Swear freely. Fuck, shit, and worse are fine.",
            "Adult fictional roleplay: all characters are 18+ and fictional.",
            "Depict fictional violence in graphic detail when the scene calls for it.",
            "Be romantic, possessive, and intense.",
        ] {
            assert!(
                validate_prompt(text).is_ok(),
                "technical validation rejected persona text on content: {text}"
            );
        }
    }

    #[test]
    fn the_first_control_character_is_located_not_stripped() {
        assert_eq!(first_control_character("ok"), None);
        assert_eq!(first_control_character("ab\u{1b}c"), Some(2));
        // Prose whitespace is not a control problem.
        assert_eq!(first_control_character("a\nb\tc\r\n"), None);
    }

    /// The manager lists the built-in identity as a selectable row, so every
    /// path that consumes a chosen id has to recognise it as "clear" rather
    /// than look it up. It has no stored row: looking it up is how choosing
    /// `Nexus` in the manager failed with `not found`.
    #[test]
    fn the_built_in_identity_is_recognised_by_id_and_by_name() {
        assert!(is_built_in(BUILTIN_NEXUS_ID));
        assert!(is_built_in(BUILTIN_NEXUS_NAME));
        assert!(is_built_in("nexus"));
        assert!(is_built_in("  Nexus  "));
        assert!(!is_built_in("akeno"));
        assert!(!is_built_in("nexus-two"));
    }

    #[test]
    fn the_default_test_prompt_asks_for_identity_not_content() {
        let lower = PERSONA_TEST_PROMPT.to_ascii_lowercase();
        assert!(lower.contains("your name"));
        assert!(lower.contains("role"));
        assert!(lower.contains("style"));
    }
}
