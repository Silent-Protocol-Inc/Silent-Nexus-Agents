//! The application's implementation of [`nexus_tools::profile::ProfilePort`].
//!
//! Every `profile.*` tool call lands here and is immediately handed to
//! [`HarnessControlPlane`], which already knows how to keep the three places
//! that record identity in step: the canonical card in the global store, the
//! active context in the workspace store, and the UI state and session row that
//! survive a restart. Reimplementing any of that closer to the tool would mean
//! two paths that agree until the day they do not.
//!
//! The other job here is wording. The tool returns a sentence and the model
//! repeats it, so this is where "created and selected", "replaced the previous
//! value", and "already stored" are decided — from what the store actually did,
//! never from what was attempted.

use crate::app::App;
use nexus_core::harness::{FactOutcome, ProfileFactStatus, ProfileStatus, UserProfile};
use nexus_core::{NexusError, Result};
use nexus_tools::profile::{FactView, Mutation, Outcome, ProfilePort, ProfileView};
use std::sync::Arc;

/// Bound to an owned [`App`] handle so it can outlive the call that built it —
/// the tool context is cloned per turn and executed on the runtime's threads.
pub struct AppProfilePort {
    app: Arc<App>,
    session_id: Option<String>,
}

impl AppProfilePort {
    /// Returns the trait object the tool context stores; there is no reason for
    /// a caller to hold the concrete type.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(app: Arc<App>, session_id: Option<String>) -> Arc<dyn ProfilePort> {
        Arc::new(Self { app, session_id })
    }

    fn session(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    fn active_id(&self) -> Result<String> {
        self.app.harness().active_profile_id(self.session())
    }

    fn view(&self, profile: &UserProfile, active_id: &str) -> Result<ProfileView> {
        let harness = self.app.harness();
        let facts = harness
            .global_repository()
            .profile_facts(&profile.id, true)?
            .into_iter()
            .map(|fact| FactView {
                id: fact.id,
                key: fact.key,
                value: match fact.value {
                    serde_json::Value::String(text) => text,
                    other => other.to_string(),
                },
                status: fact.status.as_str().to_string(),
                source: format!("{:?}", fact.source_type).to_ascii_lowercase(),
                confidence: fact.confidence,
                sensitivity: fact.sensitivity,
            })
            .collect();
        Ok(ProfileView {
            id: profile.id.clone(),
            display_name: profile.display_name.clone(),
            preferred_name: profile.preferred_name.clone(),
            aliases: profile.aliases.clone(),
            active: profile.id == active_id,
            facts,
        })
    }
}

impl ProfilePort for AppProfilePort {
    fn active(&self) -> Result<Option<ProfileView>> {
        let harness = self.app.harness();
        let active_id = self.active_id()?;
        let profile = harness.global_repository().profile(&active_id)?;
        // The placeholder card with nothing on it is not an answer to "who am I
        // talking to". Reporting it as the active profile would have the model
        // address the operator as "default".
        if profile.is_default() && profile.preferred_name.is_none() {
            let facts = harness
                .global_repository()
                .profile_facts(&active_id, true)?;
            if facts.is_empty() {
                return Ok(None);
            }
        }
        self.view(&profile, &active_id).map(Some)
    }

    fn list(&self, include_archived: bool) -> Result<Vec<ProfileView>> {
        let harness = self.app.harness();
        let active_id = self.active_id()?;
        harness
            .global_repository()
            .profiles(include_archived)?
            .iter()
            .map(|profile| self.view(profile, &active_id))
            .collect()
    }

    fn create(&self, display_name: &str, select: bool) -> Result<Mutation> {
        let harness = self.app.harness();
        // An existing card for this name is the card, not a reason to make a
        // second one. Splitting one person's history across two cards is not
        // recoverable by anything the agent can do next.
        if let Some(existing) = harness
            .global_repository()
            .profiles_named(display_name)?
            .first()
            .cloned()
        {
            if select {
                harness.select_profile(self.session(), &existing.id)?;
            }
            return Ok(Mutation {
                outcome: Outcome::Unchanged,
                profile_id: existing.id,
                fact_id: None,
                active_profile_changed: select,
                message: format!(
                    "A profile card named \u{201c}{}\u{201d} already exists{}.",
                    existing.display_name,
                    if select { " and is now selected" } else { "" }
                ),
            });
        }
        let mut profile = UserProfile::new(display_name)?;
        profile.preferred_name = Some(display_name.trim().to_string());
        harness.global_repository().create_profile(&profile)?;
        if select {
            harness.select_profile(self.session(), &profile.id)?;
        }
        Ok(Mutation {
            outcome: Outcome::Created,
            profile_id: profile.id.clone(),
            fact_id: None,
            active_profile_changed: select,
            message: if select {
                format!(
                    "Created and selected the profile card \u{201c}{}\u{201d}.",
                    profile.display_name
                )
            } else {
                format!(
                    "Created the profile card \u{201c}{}\u{201d}; it is not selected.",
                    profile.display_name
                )
            },
        })
    }

    fn select(&self, profile_id: &str) -> Result<Mutation> {
        let harness = self.app.harness();
        let profile = harness.global_repository().profile(profile_id)?;
        harness.select_profile(self.session(), profile_id)?;
        Ok(Mutation {
            outcome: Outcome::Updated,
            profile_id: profile_id.to_string(),
            fact_id: None,
            active_profile_changed: true,
            message: format!(
                "Selected the profile card \u{201c}{}\u{201d}.",
                profile.display_name
            ),
        })
    }

    fn update(&self, profile_id: &str, preferred_name: Option<&str>) -> Result<Mutation> {
        let harness = self.app.harness();
        let mut profile = harness.global_repository().profile(profile_id)?;
        let Some(name) = preferred_name.map(str::trim).filter(|n| !n.is_empty()) else {
            return Err(NexusError::ToolInput {
                tool: "profile.update".into(),
                message: "there is nothing to change".into(),
            });
        };
        if profile.preferred_name.as_deref() == Some(name) {
            return Ok(Mutation {
                outcome: Outcome::Unchanged,
                profile_id: profile.id,
                fact_id: None,
                active_profile_changed: false,
                message: format!("The profile already stores the preferred name {name}."),
            });
        }
        profile.preferred_name = Some(name.to_string());
        harness.global_repository().update_profile(&profile)?;
        Ok(Mutation {
            outcome: Outcome::Updated,
            profile_id: profile.id,
            fact_id: None,
            active_profile_changed: false,
            message: format!("Updated the profile card with the preferred name {name}."),
        })
    }

    fn add_fact(&self, key: &str, value: &str, sensitivity: &str) -> Result<Mutation> {
        let harness = self.app.harness();
        let (fact, outcome) =
            harness.record_profile_fact(self.session(), None, key, value, true, sensitivity)?;
        let pending = fact.status == ProfileFactStatus::Candidate;
        // A name recorded as a fact is also what the operator wants to be
        // called, and the card's own field is what the prompt leads with.
        if key == "identity.name" && !pending {
            let mut profile = harness.global_repository().profile(&fact.profile_id)?;
            if profile.preferred_name.is_none() {
                profile.preferred_name = Some(value.trim().to_string());
                harness.global_repository().update_profile(&profile)?;
            }
        }
        let message = match (&outcome, pending) {
            (FactOutcome::Unchanged { .. }, _) => {
                format!("The active profile already stores {key} as {value}.")
            }
            (_, true) => format!("Prepared {key} for profile review; it has not been applied yet."),
            (FactOutcome::Updated { .. }, false) => format!(
                "Updated the active profile: {key} is now {value}, replacing the previous value."
            ),
            (FactOutcome::Created { .. }, false) => {
                format!("Stored {key} as {value} on the active profile.")
            }
        };
        Ok(Mutation {
            outcome: match (&outcome, pending) {
                (FactOutcome::Unchanged { .. }, _) => Outcome::Unchanged,
                (_, true) => Outcome::RequiresReview,
                (FactOutcome::Updated { .. }, false) => Outcome::Updated,
                (FactOutcome::Created { .. }, false) => Outcome::Created,
            },
            profile_id: fact.profile_id,
            fact_id: Some(outcome.fact_id().to_string()),
            active_profile_changed: false,
            message,
        })
    }

    fn remove_fact(&self, fact_id: &str) -> Result<Mutation> {
        let harness = self.app.harness();
        let profile_id = self.active_id()?;
        harness.global_repository().set_profile_fact_status(
            &profile_id,
            fact_id,
            ProfileFactStatus::Deleted,
        )?;
        Ok(Mutation {
            outcome: Outcome::Updated,
            profile_id,
            fact_id: Some(fact_id.to_string()),
            active_profile_changed: false,
            message: "Removed the fact from the active profile.".into(),
        })
    }

    fn merge(&self, from_profile_id: &str, into_profile_id: &str) -> Result<Mutation> {
        if from_profile_id == into_profile_id {
            return Err(NexusError::ToolInput {
                tool: "profile.merge".into(),
                message: "a profile cannot be merged into itself".into(),
            });
        }
        let harness = self.app.harness();
        let repository = harness.global_repository();
        let target = repository.profile(into_profile_id)?;
        let source = repository.profile(from_profile_id)?;
        let mut moved = 0usize;
        for fact in repository.profile_facts(from_profile_id, true)? {
            let mut copy = fact.clone();
            copy.id = format!("pfact_{}", uuid::Uuid::new_v4().simple());
            copy.profile_id = target.id.clone();
            // Reconciled rather than appended: merging two cards that both say
            // the same thing must not leave the survivor saying it twice.
            if !matches!(
                repository.record_profile_fact(&copy)?,
                FactOutcome::Unchanged { .. }
            ) {
                moved += 1;
            }
            repository.set_profile_fact_status(
                from_profile_id,
                &fact.id,
                ProfileFactStatus::Superseded,
            )?;
        }
        // Archived, not deleted: the source card's provenance is the only
        // record of where the merged facts came from.
        repository.set_profile_status(from_profile_id, ProfileStatus::Archived)?;
        Ok(Mutation {
            outcome: Outcome::Updated,
            profile_id: target.id.clone(),
            fact_id: None,
            active_profile_changed: false,
            message: format!(
                "Merged {} into \u{201c}{}\u{201d} ({moved} fact{} moved) and archived the source card.",
                source.display_name,
                target.display_name,
                if moved == 1 { "" } else { "s" }
            ),
        })
    }

    fn candidates(&self) -> Result<Vec<FactView>> {
        let active_id = self.active_id()?;
        Ok(self
            .app
            .harness()
            .global_repository()
            .profile_facts(&active_id, true)?
            .into_iter()
            .filter(|fact| fact.status == ProfileFactStatus::Candidate)
            .map(|fact| FactView {
                id: fact.id,
                key: fact.key,
                value: match fact.value {
                    serde_json::Value::String(text) => text,
                    other => other.to_string(),
                },
                status: "candidate".into(),
                source: format!("{:?}", fact.source_type).to_ascii_lowercase(),
                confidence: fact.confidence,
                sensitivity: fact.sensitivity,
            })
            .collect())
    }

    fn review_candidate(&self, fact_id: &str, approve: bool) -> Result<Mutation> {
        let profile_id = self.active_id()?;
        self.app
            .harness()
            .global_repository()
            .set_profile_fact_status(
                &profile_id,
                fact_id,
                if approve {
                    ProfileFactStatus::Active
                } else {
                    ProfileFactStatus::Rejected
                },
            )?;
        Ok(Mutation {
            outcome: Outcome::Updated,
            profile_id,
            fact_id: Some(fact_id.to_string()),
            active_profile_changed: false,
            message: if approve {
                "Approved the pending fact; it is now part of the active profile.".into()
            } else {
                "Rejected the pending fact; nothing was added to the profile.".into()
            },
        })
    }
}
