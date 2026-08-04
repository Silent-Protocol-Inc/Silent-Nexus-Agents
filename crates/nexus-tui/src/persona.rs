//! PERSONA FORGE — the interactive persona editor.
//!
//! A persona is prose, sometimes pages of it, so it gets a real editor rather
//! than a slash-command argument: multiline entry, paste, cursor movement,
//! undo/redo, a live character and token estimate, and a raw view showing the
//! exact text that will be stored and sent. Nothing here composes a command
//! line for the operator to finish typing — the previous manager pasted
//! `/persona create name instructions` into the message composer, which is how
//! a persona ended up feeling like a label instead of an identity.
//!
//! The forge never inspects what the persona *says*. It validates length,
//! encoding, and control characters through
//! [`nexus_app::persona_service::validate_prompt`] and stores the rest exactly
//! as written.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nexus_core::persona::{ContentProfile, InheritanceMode, PersistencePolicy};

/// A plain multiline text buffer with a byte cursor and an undo history.
///
/// Equality compares the visible buffer only: two editors holding the same
/// prose are the same draft regardless of how many undo steps got there.
#[derive(Debug, Clone, Default)]
pub struct MultilineEditor {
    text: String,
    cursor: usize,
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    /// Row the viewport starts at, so a long persona scrolls instead of
    /// clipping the line being typed.
    pub scroll: usize,
}

/// Undo granularity: a run of ordinary typing collapses into one step so undo
/// does not walk back character by character.
const UNDO_LIMIT: usize = 200;

impl PartialEq for MultilineEditor {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.cursor == other.cursor
    }
}

impl MultilineEditor {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self {
            text,
            cursor,
            ..Self::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    /// Rough token estimate, matching the harness's own 4-chars-per-token rule
    /// so the number shown here and the number budgeted later agree.
    pub fn token_estimate(&self) -> usize {
        self.text.len().div_ceil(4)
    }

    pub fn line_count(&self) -> usize {
        self.text.lines().count().max(1)
    }

    /// Zero-based (row, column-in-chars) of the cursor.
    pub fn position(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let row = before.matches('\n').count();
        let column = before
            .rsplit('\n')
            .next()
            .map(|line| line.chars().count())
            .unwrap_or(0);
        (row, column)
    }

    fn checkpoint(&mut self) {
        self.undo.push((self.text.clone(), self.cursor));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.checkpoint();
        self.text = text.into();
        self.cursor = self.cursor.min(self.text.len());
        while !self.text.is_char_boundary(self.cursor) {
            self.cursor -= 1;
        }
    }

    pub fn insert(&mut self, ch: char) {
        // Typing runs collapse: only start a new undo step at a word or line
        // boundary, so Ctrl+Z removes a word rather than a keystroke.
        if ch.is_whitespace() || self.undo.is_empty() {
            self.checkpoint();
        }
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    pub fn insert_paste(&mut self, pasted: &str) {
        self.checkpoint();
        self.text.insert_str(self.cursor, pasted);
        self.cursor += pasted.len();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.checkpoint();
        let previous = self.text[..self.cursor]
            .chars()
            .next_back()
            .map(char::len_utf8)
            .unwrap_or(1);
        self.cursor -= previous;
        self.text.remove(self.cursor);
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        self.checkpoint();
        self.text.remove(self.cursor);
    }

    pub fn left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= self.text[..self.cursor]
                .chars()
                .next_back()
                .map(char::len_utf8)
                .unwrap_or(1);
        }
    }

    pub fn right(&mut self) {
        if let Some(ch) = self.text[self.cursor..].chars().next() {
            self.cursor += ch.len_utf8();
        }
    }

    pub fn line_home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map(|at| at + 1)
            .unwrap_or(0);
    }

    pub fn line_end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map(|at| self.cursor + at)
            .unwrap_or(self.text.len());
    }

    pub fn up(&mut self) {
        let (row, column) = self.position();
        if row == 0 {
            self.cursor = 0;
            return;
        }
        self.move_to(row - 1, column);
    }

    pub fn down(&mut self) {
        let (row, column) = self.position();
        if row + 1 >= self.line_count() {
            self.cursor = self.text.len();
            return;
        }
        self.move_to(row + 1, column);
    }

    fn move_to(&mut self, row: usize, column: usize) {
        let mut offset = 0usize;
        for (index, line) in self.text.split('\n').enumerate() {
            if index == row {
                let within: usize = line.chars().take(column).map(char::len_utf8).sum();
                self.cursor = offset + within;
                return;
            }
            offset += line.len() + 1;
        }
        self.cursor = self.text.len();
    }

    pub fn undo(&mut self) -> bool {
        let Some((text, cursor)) = self.undo.pop() else {
            return false;
        };
        self.redo.push((self.text.clone(), self.cursor));
        self.text = text;
        self.cursor = cursor.min(self.text.len());
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some((text, cursor)) = self.redo.pop() else {
            return false;
        };
        self.undo.push((self.text.clone(), self.cursor));
        self.text = text;
        self.cursor = cursor.min(self.text.len());
        true
    }

    /// Keep the cursor's row inside a viewport `height` rows tall.
    pub fn follow_cursor(&mut self, height: usize) {
        let (row, _) = self.position();
        if row < self.scroll {
            self.scroll = row;
        } else if height > 0 && row >= self.scroll + height {
            self.scroll = row + 1 - height;
        }
    }
}

/// Sections offered by the structured editor. Every one is optional; an empty
/// section contributes nothing to the composed prompt.
pub const STRUCTURED_SECTIONS: &[&str] = &[
    "Identity",
    "Role",
    "Personality",
    "Tone",
    "Relationship framing",
    "Communication style",
    "Response behavior",
    "Content preferences",
    "Roleplay behavior",
    "Formatting",
    "Examples",
    "Custom instructions",
];

/// Which step of the wizard is showing. Narrow terminals show one step at a
/// time; wide ones show the same steps with more of each visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeStage {
    Identity,
    Base,
    Instructions,
    Metadata,
    Review,
}

impl ForgeStage {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Base => "base persona",
            Self::Instructions => "persona instructions",
            Self::Metadata => "metadata",
            Self::Review => "review",
        }
    }
}

/// One selectable base persona.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseChoice {
    pub id: String,
    pub name: String,
    pub content_profile: ContentProfile,
}

/// How the review step will finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeCommit {
    CreateAndActivate,
    CreateOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersonaForge {
    /// Set when editing an existing persona; `None` when creating.
    pub editing: Option<String>,
    pub stage: ForgeStage,
    pub name: String,
    pub description: String,
    /// The raw prompt — always the authority. The structured sections compose
    /// into it, never around it.
    pub raw: MultilineEditor,
    pub structured: bool,
    pub sections: Vec<(&'static str, String)>,
    pub section_focus: usize,
    pub bases: Vec<BaseChoice>,
    /// Index into `bases`, or `None` for "no base".
    pub base: Option<usize>,
    pub inheritance: InheritanceMode,
    pub content_profile: ContentProfile,
    pub category: String,
    pub tags: String,
    pub compatibility_notes: String,
    pub persistence: PersistencePolicy,
    pub adult_acknowledged: bool,
    pub commit: ForgeCommit,
    /// Field index within the current stage.
    pub focus: usize,
    pub error: Option<String>,
    pub dirty: bool,
    /// True once Esc was pressed with unsaved work; a second Esc discards.
    pub confirm_discard: bool,
}

impl PersonaForge {
    pub fn create(bases: Vec<BaseChoice>) -> Self {
        Self {
            editing: None,
            stage: ForgeStage::Identity,
            name: String::new(),
            description: String::new(),
            raw: MultilineEditor::default(),
            structured: false,
            sections: STRUCTURED_SECTIONS
                .iter()
                .map(|name| (*name, String::new()))
                .collect(),
            section_focus: 0,
            bases,
            base: None,
            inheritance: InheritanceMode::Snapshot,
            content_profile: ContentProfile::General,
            category: String::new(),
            tags: String::new(),
            compatibility_notes: String::new(),
            persistence: PersistencePolicy::Persistent,
            adult_acknowledged: false,
            commit: ForgeCommit::CreateAndActivate,
            focus: 0,
            error: None,
            dirty: false,
            confirm_discard: false,
        }
    }

    /// Open the forge on an existing persona. Its stored text is loaded into
    /// the raw editor verbatim.
    pub fn edit(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        prompt: impl Into<String>,
        content_profile: ContentProfile,
        bases: Vec<BaseChoice>,
    ) -> Self {
        let mut forge = Self::create(bases);
        forge.editing = Some(id.into());
        forge.name = name.into();
        forge.description = description.into();
        forge.raw = MultilineEditor::new(prompt);
        forge.content_profile = content_profile;
        forge.adult_acknowledged = content_profile.requires_acknowledgment();
        forge.commit = ForgeCommit::CreateOnly;
        forge
    }

    pub fn header(&self) -> String {
        if self.editing.is_some() {
            "PERSONA FORGE // EDIT".into()
        } else {
            "PERSONA FORGE // CREATE".into()
        }
    }

    /// The stages actually shown. The base step is hidden outright when no
    /// persona could serve as a base — an empty disabled selector teaches
    /// nothing and costs a keystroke.
    pub fn stages(&self) -> Vec<ForgeStage> {
        let mut stages = vec![ForgeStage::Identity];
        if !self.bases.is_empty() {
            stages.push(ForgeStage::Base);
        }
        stages.extend([
            ForgeStage::Instructions,
            ForgeStage::Metadata,
            ForgeStage::Review,
        ]);
        stages
    }

    pub fn stage_index(&self) -> usize {
        self.stages()
            .iter()
            .position(|stage| *stage == self.stage)
            .unwrap_or(0)
    }

    pub fn next_stage(&mut self) {
        let stages = self.stages();
        let index = (self.stage_index() + 1).min(stages.len() - 1);
        self.stage = stages[index];
        self.focus = 0;
    }

    pub fn previous_stage(&mut self) {
        let stages = self.stages();
        let index = self.stage_index().saturating_sub(1);
        self.stage = stages[index];
        self.focus = 0;
    }

    /// The exact text that will be stored and sent.
    pub fn prompt(&self) -> String {
        if self.structured {
            compose_sections(&self.sections)
        } else {
            self.raw.text().to_string()
        }
    }

    /// Move between the raw and structured views without losing a character.
    ///
    /// Structured → raw composes the sections. Raw → structured keeps the
    /// sections when they still compose to the same text, and otherwise parks
    /// the whole raw prompt in `Custom instructions` — text the operator typed
    /// is never dropped to make a view tidier.
    pub fn toggle_structured(&mut self) {
        if self.structured {
            let composed = compose_sections(&self.sections);
            self.raw = MultilineEditor::new(composed);
            self.structured = false;
        } else {
            let raw = self.raw.text().to_string();
            if compose_sections(&self.sections) != raw {
                for (_, value) in self.sections.iter_mut() {
                    value.clear();
                }
                if let Some(slot) = self
                    .sections
                    .iter_mut()
                    .find(|(name, _)| *name == "Custom instructions")
                {
                    slot.1 = raw;
                }
            }
            self.structured = true;
        }
        self.dirty = true;
    }

    pub fn base_id(&self) -> Option<String> {
        self.base
            .and_then(|index| self.bases.get(index))
            .map(|base| base.id.clone())
    }

    /// Everything blocking a save, in the order an operator should fix it.
    pub fn validation_error(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("a persona needs a name".into());
        }
        let prompt = self.prompt();
        if let Err(error) = nexus_app::persona_service::validate_prompt(&prompt) {
            return Some(error.to_string());
        }
        if self.content_profile.requires_acknowledgment() && !self.adult_acknowledged {
            return Some(
                "adults-only personas need the acknowledgment on the metadata step".into(),
            );
        }
        None
    }

    pub fn spec(&self) -> nexus_app::persona_service::PersonaSpec {
        nexus_app::persona_service::PersonaSpec {
            name: self.name.trim().to_string(),
            description: self.description.trim().to_string(),
            instructions: self.prompt(),
            scope: "project".into(),
            base_persona_id: self.base_id(),
            inheritance_mode: self.inheritance,
            content_profile: self.content_profile,
            category: self.category.trim().to_string(),
            tags: split_tags(&self.tags),
            compatibility_notes: self.compatibility_notes.trim().to_string(),
            recommended_providers: Vec::new(),
            recommended_models: Vec::new(),
            recommended_agents: Vec::new(),
            persistence_policy: self.persistence,
            adult_acknowledged: self.adult_acknowledged,
            activate: matches!(self.commit, ForgeCommit::CreateAndActivate),
        }
    }

    /// The single-line fields of the current stage, as (label, value) pairs.
    pub fn stage_fields(&self) -> Vec<(&'static str, String)> {
        match self.stage {
            ForgeStage::Identity => vec![
                ("name", self.name.clone()),
                ("description", self.description.clone()),
            ],
            ForgeStage::Metadata => vec![
                ("content profile", self.content_profile.label().to_string()),
                ("category", self.category.clone()),
                ("tags", self.tags.clone()),
                ("compatibility notes", self.compatibility_notes.clone()),
                ("persistence", self.persistence.as_str().to_string()),
                (
                    "adult acknowledgment",
                    if !self.content_profile.requires_acknowledgment() {
                        "not required".into()
                    } else if self.adult_acknowledged {
                        "given".into()
                    } else {
                        "required — press space".into()
                    },
                ),
            ],
            _ => Vec::new(),
        }
    }

    fn field_mut(&mut self, index: usize) -> Option<&mut String> {
        match (self.stage, index) {
            (ForgeStage::Identity, 0) => Some(&mut self.name),
            (ForgeStage::Identity, 1) => Some(&mut self.description),
            (ForgeStage::Metadata, 1) => Some(&mut self.category),
            (ForgeStage::Metadata, 2) => Some(&mut self.tags),
            (ForgeStage::Metadata, 3) => Some(&mut self.compatibility_notes),
            _ => None,
        }
    }

    fn cycle_choice(&mut self, forward: bool) {
        match (self.stage, self.focus) {
            (ForgeStage::Base, 0) => {
                let count = self.bases.len() + 1;
                let current = self.base.map(|index| index + 1).unwrap_or(0);
                let next = if forward {
                    (current + 1) % count
                } else {
                    (current + count - 1) % count
                };
                self.base = (next > 0).then(|| next - 1);
                if self.base.is_none() {
                    self.inheritance = InheritanceMode::Snapshot;
                }
            }
            (ForgeStage::Base, 1) => {
                self.inheritance = match self.inheritance {
                    InheritanceMode::Snapshot => InheritanceMode::Extend,
                    InheritanceMode::Extend => InheritanceMode::Snapshot,
                };
            }
            (ForgeStage::Metadata, 0) => {
                let profiles = [
                    ContentProfile::General,
                    ContentProfile::Mature,
                    ContentProfile::AdultsOnly,
                    ContentProfile::Custom,
                ];
                let current = profiles
                    .iter()
                    .position(|profile| *profile == self.content_profile)
                    .unwrap_or(0);
                let next = if forward {
                    (current + 1) % profiles.len()
                } else {
                    (current + profiles.len() - 1) % profiles.len()
                };
                self.content_profile = profiles[next];
            }
            (ForgeStage::Metadata, 4) => {
                self.persistence = match self.persistence {
                    PersistencePolicy::Persistent => PersistencePolicy::SessionOnly,
                    PersistencePolicy::SessionOnly => PersistencePolicy::Persistent,
                };
            }
            (ForgeStage::Metadata, 5) => {
                if self.content_profile.requires_acknowledgment() {
                    self.adult_acknowledged = !self.adult_acknowledged;
                }
            }
            (ForgeStage::Review, _) => {
                self.commit = match self.commit {
                    ForgeCommit::CreateAndActivate => ForgeCommit::CreateOnly,
                    ForgeCommit::CreateOnly => ForgeCommit::CreateAndActivate,
                };
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn field_count(&self) -> usize {
        match self.stage {
            ForgeStage::Identity => 2,
            ForgeStage::Base => 2,
            ForgeStage::Instructions => 1,
            ForgeStage::Metadata => 6,
            ForgeStage::Review => 1,
        }
    }

    /// True when the instructions editor owns the keyboard.
    fn editing_prompt(&self) -> bool {
        matches!(self.stage, ForgeStage::Instructions)
    }

    fn active_prompt_editor(&mut self) -> Option<&mut MultilineEditor> {
        (!self.structured).then_some(&mut self.raw)
    }

    fn structured_value(&mut self) -> Option<&mut String> {
        let index = self.section_focus;
        self.sections.get_mut(index).map(|(_, value)| value)
    }
}

/// Compose the structured sections into one prompt.
///
/// Headers are emitted only for sections with content, so a persona that used
/// two sections does not ship ten empty headings to the model.
pub fn compose_sections(sections: &[(&'static str, String)]) -> String {
    let mut out = String::new();
    for (name, value) in sections {
        if value.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        // The catch-all section carries the operator's own text with no heading
        // added on top of it.
        if *name == "Custom instructions" {
            out.push_str(value.trim_end());
        } else {
            out.push_str(name);
            out.push_str(":\n");
            out.push_str(value.trim_end());
        }
    }
    out
}

fn split_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

/// What a key press asked the surrounding overlay to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ForgeOutcome {
    Consumed,
    /// Close without saving.
    Cancel,
    /// Save; the boolean is whether to activate it immediately.
    Submit(Box<nexus_app::persona_service::PersonaSpec>),
}

impl PersonaForge {
    pub fn handle_key(&mut self, key: KeyEvent) -> ForgeOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        self.error = None;
        match key.code {
            KeyCode::Esc => {
                // Unsaved work is never discarded on a single key. The first
                // Esc asks; only a second one throws the draft away.
                if self.dirty && !self.confirm_discard {
                    self.confirm_discard = true;
                    self.error = Some("unsaved changes — press Esc again to discard".into());
                    return ForgeOutcome::Consumed;
                }
                return ForgeOutcome::Cancel;
            }
            KeyCode::Char('s') if ctrl => return self.submit(),
            KeyCode::Char('r') if ctrl => {
                self.toggle_structured();
                return ForgeOutcome::Consumed;
            }
            KeyCode::Char('z') if ctrl => {
                if let Some(editor) = self.active_prompt_editor() {
                    editor.undo();
                }
                return ForgeOutcome::Consumed;
            }
            KeyCode::Char('y') if ctrl => {
                if let Some(editor) = self.active_prompt_editor() {
                    editor.redo();
                }
                return ForgeOutcome::Consumed;
            }
            KeyCode::PageDown => {
                self.next_stage();
                return ForgeOutcome::Consumed;
            }
            KeyCode::PageUp => {
                self.previous_stage();
                return ForgeOutcome::Consumed;
            }
            _ => {}
        }
        self.confirm_discard = false;

        if self.editing_prompt() {
            return self.handle_prompt_key(key);
        }

        match key.code {
            KeyCode::Tab | KeyCode::Down => {
                self.focus = (self.focus + 1) % self.field_count();
                ForgeOutcome::Consumed
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.focus = (self.focus + self.field_count() - 1) % self.field_count();
                ForgeOutcome::Consumed
            }
            KeyCode::Left => {
                self.cycle_choice(false);
                ForgeOutcome::Consumed
            }
            KeyCode::Right | KeyCode::Char(' ') if self.field_mut(self.focus).is_none() => {
                self.cycle_choice(true);
                ForgeOutcome::Consumed
            }
            KeyCode::Enter => {
                if matches!(self.stage, ForgeStage::Review) {
                    self.submit()
                } else {
                    self.next_stage();
                    ForgeOutcome::Consumed
                }
            }
            KeyCode::Backspace => {
                let focus = self.focus;
                if let Some(field) = self.field_mut(focus) {
                    field.pop();
                    self.dirty = true;
                }
                ForgeOutcome::Consumed
            }
            KeyCode::Char(ch) => {
                let focus = self.focus;
                if let Some(field) = self.field_mut(focus) {
                    field.push(ch);
                    self.dirty = true;
                } else {
                    self.cycle_choice(true);
                }
                ForgeOutcome::Consumed
            }
            _ => ForgeOutcome::Consumed,
        }
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) -> ForgeOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.structured {
            match key.code {
                KeyCode::Tab => {
                    self.section_focus = (self.section_focus + 1) % self.sections.len();
                    return ForgeOutcome::Consumed;
                }
                KeyCode::BackTab => {
                    self.section_focus =
                        (self.section_focus + self.sections.len() - 1) % self.sections.len();
                    return ForgeOutcome::Consumed;
                }
                KeyCode::Backspace => {
                    if let Some(value) = self.structured_value() {
                        value.pop();
                    }
                    self.dirty = true;
                    return ForgeOutcome::Consumed;
                }
                KeyCode::Enter => {
                    if let Some(value) = self.structured_value() {
                        value.push('\n');
                    }
                    self.dirty = true;
                    return ForgeOutcome::Consumed;
                }
                KeyCode::Char(ch) if !ctrl => {
                    if let Some(value) = self.structured_value() {
                        value.push(ch);
                    }
                    self.dirty = true;
                    return ForgeOutcome::Consumed;
                }
                _ => return ForgeOutcome::Consumed,
            }
        }
        match key.code {
            // Enter writes a newline: this is an editor, not a form field, and
            // a persona that cannot contain a paragraph break is not a persona.
            KeyCode::Enter => {
                self.raw.insert('\n');
                self.dirty = true;
            }
            KeyCode::Backspace => {
                self.raw.backspace();
                self.dirty = true;
            }
            KeyCode::Delete => {
                self.raw.delete();
                self.dirty = true;
            }
            KeyCode::Left => self.raw.left(),
            KeyCode::Right => self.raw.right(),
            KeyCode::Up => self.raw.up(),
            KeyCode::Down => self.raw.down(),
            KeyCode::Home => self.raw.line_home(),
            KeyCode::End => self.raw.line_end(),
            KeyCode::Tab => {
                self.raw.insert('\t');
                self.dirty = true;
            }
            KeyCode::Char(ch) if !ctrl => {
                self.raw.insert(ch);
                self.dirty = true;
            }
            _ => {}
        }
        ForgeOutcome::Consumed
    }

    pub fn paste(&mut self, text: &str) {
        if self.editing_prompt() && !self.structured {
            self.raw.insert_paste(text);
        } else if self.editing_prompt() {
            if let Some(value) = self.structured_value() {
                value.push_str(text);
            }
        } else {
            let focus = self.focus;
            let single_line = text.replace('\n', " ");
            if let Some(field) = self.field_mut(focus) {
                field.push_str(&single_line);
            }
        }
        self.dirty = true;
    }

    fn submit(&mut self) -> ForgeOutcome {
        if let Some(error) = self.validation_error() {
            self.error = Some(error);
            return ForgeOutcome::Consumed;
        }
        ForgeOutcome::Submit(Box::new(self.spec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    #[test]
    fn the_editor_handles_multiple_lines_and_cursor_movement() {
        let mut editor = MultilineEditor::new("alpha\nbeta\ngamma");
        assert_eq!(editor.line_count(), 3);
        editor.line_home();
        assert_eq!(editor.position(), (2, 0));
        editor.up();
        assert_eq!(editor.position(), (1, 0));
        editor.line_end();
        assert_eq!(editor.position(), (1, 4));
        editor.insert('!');
        assert_eq!(editor.text(), "alpha\nbeta!\ngamma");
    }

    #[test]
    fn paste_and_undo_work_on_the_prompt() {
        let mut editor = MultilineEditor::new("");
        editor.insert_paste("pasted persona text");
        assert_eq!(editor.text(), "pasted persona text");
        assert!(editor.undo());
        assert_eq!(editor.text(), "");
        assert!(editor.redo());
        assert_eq!(editor.text(), "pasted persona text");
    }

    #[test]
    fn enter_writes_a_newline_instead_of_submitting() {
        let mut forge = PersonaForge::create(Vec::new());
        forge.stage = ForgeStage::Instructions;
        forge.handle_key(key(KeyCode::Char('a')));
        forge.handle_key(key(KeyCode::Enter));
        forge.handle_key(key(KeyCode::Char('b')));
        assert_eq!(forge.raw.text(), "a\nb");
    }

    #[test]
    fn the_base_step_is_hidden_when_nothing_can_be_a_base() {
        let empty = PersonaForge::create(Vec::new());
        assert!(!empty.stages().contains(&ForgeStage::Base));
        let with_base = PersonaForge::create(vec![BaseChoice {
            id: "p1".into(),
            name: "odysseus".into(),
            content_profile: ContentProfile::General,
        }]);
        assert!(with_base.stages().contains(&ForgeStage::Base));
    }

    #[test]
    fn switching_editors_never_loses_text() {
        let mut forge = PersonaForge::create(Vec::new());
        forge.stage = ForgeStage::Instructions;
        forge.raw = MultilineEditor::new("handwritten persona prose");
        forge.toggle_structured();
        assert!(forge.structured);
        // The prose survived the move into the structured view…
        assert!(forge.prompt().contains("handwritten persona prose"));
        forge.toggle_structured();
        // …and back out of it.
        assert!(forge.raw.text().contains("handwritten persona prose"));
    }

    #[test]
    fn structured_sections_compose_only_what_was_filled_in() {
        let sections = vec![
            ("Identity", "You are Odysseus.".to_string()),
            ("Tone", String::new()),
            ("Custom instructions", "Never break character.".to_string()),
        ];
        let composed = compose_sections(&sections);
        assert!(composed.starts_with("Identity:\nYou are Odysseus."));
        assert!(composed.contains("Never break character."));
        assert!(!composed.contains("Tone"));
    }

    #[test]
    fn unsaved_work_survives_the_first_escape() {
        let mut forge = PersonaForge::create(Vec::new());
        forge.stage = ForgeStage::Instructions;
        forge.handle_key(key(KeyCode::Char('x')));
        assert_eq!(forge.handle_key(key(KeyCode::Esc)), ForgeOutcome::Consumed);
        assert!(forge.confirm_discard);
        assert_eq!(forge.handle_key(key(KeyCode::Esc)), ForgeOutcome::Cancel);
    }

    #[test]
    fn saving_requires_a_name_and_a_prompt_but_nothing_about_content() {
        let mut forge = PersonaForge::create(Vec::new());
        forge.stage = ForgeStage::Instructions;
        forge.raw = MultilineEditor::new("Explicit adult fiction; swear freely.");
        assert!(matches!(
            forge.handle_key(ctrl('s')),
            ForgeOutcome::Consumed
        ));
        assert_eq!(
            forge.validation_error().as_deref(),
            Some("a persona needs a name")
        );
        forge.name = "odysseus".into();
        // The text is mature and that is not a validation concern.
        assert_eq!(forge.validation_error(), None);
        let ForgeOutcome::Submit(spec) = forge.handle_key(ctrl('s')) else {
            panic!("a named persona with text must save");
        };
        assert_eq!(spec.instructions, "Explicit adult fiction; swear freely.");
    }

    #[test]
    fn adults_only_needs_the_acknowledgment_before_it_can_save() {
        let mut forge = PersonaForge::create(Vec::new());
        forge.name = "odysseus".into();
        forge.raw = MultilineEditor::new("adult fiction");
        forge.content_profile = ContentProfile::AdultsOnly;
        assert!(forge
            .validation_error()
            .is_some_and(|error| error.contains("acknowledgment")));
        forge.adult_acknowledged = true;
        assert_eq!(forge.validation_error(), None);
        // The acknowledgment is metadata; it changed no text.
        assert_eq!(forge.prompt(), "adult fiction");
    }

    #[test]
    fn the_forge_never_produces_a_command_line() {
        let mut forge = PersonaForge::create(Vec::new());
        forge.name = "odysseus".into();
        forge.raw = MultilineEditor::new("be brief");
        let spec = forge.spec();
        assert!(!spec.instructions.starts_with('/'));
        assert!(!spec.instructions.contains("persona create"));
    }
}
