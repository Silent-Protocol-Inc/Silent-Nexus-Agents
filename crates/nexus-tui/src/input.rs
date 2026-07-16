//! The main input editor: cursor movement, history recall, and multiline
//! support (Alt+Enter inserts a newline; Enter submits).

/// Line editor state for the input box.
#[derive(Default)]
pub struct InputEditor {
    text: String,
    /// Byte offset of the cursor (always on a char boundary).
    cursor: usize,
    /// Committed history (oldest first) and the current browse position.
    history: Vec<String>,
    browse: Option<usize>,
    /// The in-progress line stashed while browsing history.
    stash: String,
}

impl InputEditor {
    pub fn with_history(history: Vec<String>) -> Self {
        Self {
            history,
            ..Default::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.browse = None;
    }

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.browse = None;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = prev_boundary(&self.text, self.cursor);
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let next = next_boundary(&self.text, self.cursor);
        self.text.replace_range(self.cursor..next, "");
    }

    pub fn left(&mut self) {
        if self.cursor > 0 {
            self.cursor = prev_boundary(&self.text, self.cursor);
        }
    }

    pub fn right(&mut self) {
        if self.cursor < self.text.len() {
            self.cursor = next_boundary(&self.text, self.cursor);
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }

    /// Delete the word before the cursor (Ctrl+W).
    pub fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        let start = trimmed
            .rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// Clear the whole line (Ctrl+U).
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.browse = None;
    }

    /// Take the current text for submission, committing it to history.
    pub fn take(&mut self) -> String {
        let line = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.browse = None;
        if !line.trim().is_empty() && self.history.last() != Some(&line) {
            self.history.push(line.clone());
        }
        line
    }

    /// Browse one step back in history (Up).
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next_index = match self.browse {
            None => {
                self.stash = self.text.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.browse = Some(next_index);
        self.text = self.history[next_index].clone();
        self.cursor = self.text.len();
    }

    /// Browse one step forward (Down); past the newest entry restores the stash.
    pub fn history_next(&mut self) {
        let Some(i) = self.browse else { return };
        if i + 1 < self.history.len() {
            self.browse = Some(i + 1);
            self.text = self.history[i + 1].clone();
        } else {
            self.browse = None;
            self.text = std::mem::take(&mut self.stash);
        }
        self.cursor = self.text.len();
    }

    pub fn history_snapshot(&self) -> &[String] {
        &self.history
    }
}

fn prev_boundary(s: &str, from: usize) -> usize {
    let mut i = from - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_boundary(s: &str, from: usize) -> usize {
    let mut i = from + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_move_delete() {
        let mut e = InputEditor::default();
        for c in "héllo".chars() {
            e.insert(c);
        }
        assert_eq!(e.text(), "héllo");
        e.left();
        e.left();
        e.backspace(); // remove the first l
        assert_eq!(e.text(), "hélo");
        e.home();
        e.delete();
        assert_eq!(e.text(), "élo");
        e.end();
        e.insert('!');
        assert_eq!(e.text(), "élo!");
    }

    #[test]
    fn word_delete_and_clear() {
        let mut e = InputEditor::default();
        e.set_text("goal create fix parser");
        e.delete_word();
        assert_eq!(e.text(), "goal create fix ");
        e.delete_word();
        assert_eq!(e.text(), "goal create ");
        e.clear();
        assert!(e.is_empty());
    }

    #[test]
    fn history_roundtrip() {
        let mut e = InputEditor::default();
        e.set_text("/status");
        assert_eq!(e.take(), "/status");
        e.set_text("/goals");
        assert_eq!(e.take(), "/goals");

        e.set_text("draf");
        e.history_prev();
        assert_eq!(e.text(), "/goals");
        e.history_prev();
        assert_eq!(e.text(), "/status");
        e.history_prev(); // stays at oldest
        assert_eq!(e.text(), "/status");
        e.history_next();
        assert_eq!(e.text(), "/goals");
        e.history_next(); // restores the stashed draft
        assert_eq!(e.text(), "draf");
    }

    #[test]
    fn duplicate_history_entries_are_collapsed() {
        let mut e = InputEditor::default();
        e.set_text("/status");
        e.take();
        e.set_text("/status");
        e.take();
        assert_eq!(e.history_snapshot().len(), 1);
    }
}
