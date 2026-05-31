//! In-house editable text field for the TUI.
//!
//! Provides a UTF-8-safe, cursor-aware editor used by the message input, the
//! command line, and overlay text fields (e.g. password entry). A small
//! hand-rolled editor is used instead of `tui-textarea` because that crate
//! pins `ratatui 0.29`, which is incompatible with this project's `ratatui 0.30`.
//!
//! The cursor is tracked as a char index into a `Vec<char>` so insert/delete are
//! simple and never split a multi-byte code point. Display columns use
//! `unicode-width` so wide (CJK/emoji) glyphs position the terminal cursor
//! correctly.

use ratatui_crossterm::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

/// A single- or multi-line editable text buffer with a char-index cursor.
#[derive(Clone, Default)]
pub struct EditableField {
    chars: Vec<char>,
    /// Cursor position as a char index in `[0, chars.len()]`.
    cursor: usize,
    multiline: bool,
    /// Render the contents as `•` (used for password entry).
    masked: bool,
    /// Previously submitted entries (oldest first); used for history recall.
    history: Vec<String>,
    /// Index into `history` while browsing, or `None` when editing live text.
    history_pos: Option<usize>,
    /// Live text stashed while browsing history so it can be restored.
    stash: Option<String>,
}

impl EditableField {
    pub fn new(multiline: bool) -> Self {
        Self {
            multiline,
            ..Default::default()
        }
    }

    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    /// Current contents as a `String`.
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Text suitable for rendering (masked fields return `•` per char,
    /// preserving newlines).
    pub fn display_text(&self) -> String {
        if self.masked {
            self.chars
                .iter()
                .map(|&c| if c == '\n' { '\n' } else { '•' })
                .collect()
        } else {
            self.text()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.history_pos = None;
        self.stash = None;
    }

    pub fn set_text(&mut self, text: &str) {
        self.put(text);
        self.history_pos = None;
        self.stash = None;
    }

    /// Replace the buffer contents without disturbing history-browsing state.
    fn put(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' && !self.multiline {
            return;
        }
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert_char(c);
        }
    }

    pub fn newline(&mut self) {
        if self.multiline {
            self.insert_char('\n');
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    pub fn delete_forward(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    /// Delete the word (and preceding whitespace) before the cursor (Ctrl+W).
    pub fn delete_word_back(&mut self) {
        let mut end = self.cursor;
        while end > 0 && self.chars[end - 1].is_whitespace() {
            end -= 1;
        }
        while end > 0 && !self.chars[end - 1].is_whitespace() {
            end -= 1;
        }
        self.chars.drain(end..self.cursor);
        self.cursor = end;
    }

    /// Delete from the start of the current line to the cursor (Ctrl+U).
    pub fn delete_to_line_start(&mut self) {
        let mut start = self.cursor;
        while start > 0 && self.chars[start - 1] != '\n' {
            start -= 1;
        }
        self.chars.drain(start..self.cursor);
        self.cursor = start;
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
    }

    pub fn move_home(&mut self) {
        while self.cursor > 0 && self.chars[self.cursor - 1] != '\n' {
            self.cursor -= 1;
        }
    }

    pub fn move_end(&mut self) {
        while self.cursor < self.chars.len() && self.chars[self.cursor] != '\n' {
            self.cursor += 1;
        }
    }

    /// Cursor position for rendering: `(row, col)` in display cells, relative to
    /// the start of the text. `col` accounts for wide glyphs.
    pub fn cursor_display(&self) -> (u16, u16) {
        let mut row = 0u16;
        let mut col = 0u16;
        for &c in self.chars.iter().take(self.cursor) {
            if c == '\n' {
                row = row.saturating_add(1);
                col = 0;
            } else {
                let w = UnicodeWidthChar::width(c).unwrap_or(0) as u16;
                col = col.saturating_add(w);
            }
        }
        (row, col)
    }

    // ----- command-history support (single-line use) -----

    /// Record a submitted entry for later recall. No-op for blank/duplicate.
    pub fn push_history(&mut self, entry: &str) {
        if entry.trim().is_empty() {
            return;
        }
        if self.history.last().map(String::as_str) == Some(entry) {
            return;
        }
        self.history.push(entry.to_string());
    }

    /// Recall the previous (older) history entry into the buffer.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_pos {
            None => {
                self.stash = Some(self.text());
                self.history_pos = Some(self.history.len() - 1);
            }
            Some(0) => {}
            Some(p) => self.history_pos = Some(p - 1),
        }
        if let Some(p) = self.history_pos {
            let entry = self.history[p].clone();
            self.put(&entry);
        }
    }

    /// Recall the next (newer) history entry, or restore the stashed live text.
    pub fn history_next(&mut self) {
        let Some(p) = self.history_pos else {
            return;
        };
        if p + 1 < self.history.len() {
            let entry = self.history[p + 1].clone();
            self.put(&entry);
            self.history_pos = Some(p + 1);
        } else {
            let stash = self.stash.take().unwrap_or_default();
            self.put(&stash);
            self.history_pos = None;
        }
    }

    /// Handle a pure-editing key. Returns `true` if the key was consumed.
    /// Submission keys (Enter), cancellation (Esc), Tab, and newline insertion
    /// are intentionally left to the caller because their meaning is contextual.
    pub fn handle_edit_key(&mut self, key: &KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('w') if ctrl => self.delete_word_back(),
            KeyCode::Char('u') if ctrl => self.delete_to_line_start(),
            KeyCode::Char('a') if ctrl => self.move_home(),
            KeyCode::Char('e') if ctrl => self.move_end(),
            KeyCode::Char(c) if !ctrl => self.insert_char(c),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete_forward(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Home => self.move_home(),
            KeyCode::End => self.move_end(),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_backspace_are_utf8_safe() {
        let mut f = EditableField::new(false);
        f.insert_str("héllo👋");
        assert_eq!(f.text(), "héllo👋");
        f.backspace();
        assert_eq!(f.text(), "héllo");
    }

    #[test]
    fn cursor_movement_and_mid_string_edit() {
        let mut f = EditableField::new(false);
        f.set_text("abcd");
        f.move_left();
        f.move_left();
        f.insert_char('X'); // ab X cd
        assert_eq!(f.text(), "abXcd");
    }

    #[test]
    fn delete_word_back_removes_trailing_word() {
        let mut f = EditableField::new(false);
        f.set_text("hello world");
        f.delete_word_back();
        assert_eq!(f.text(), "hello ");
    }

    #[test]
    fn newline_only_in_multiline() {
        let mut single = EditableField::new(false);
        single.insert_char('\n');
        assert_eq!(single.text(), "");

        let mut multi = EditableField::new(true);
        multi.insert_str("a");
        multi.newline();
        multi.insert_str("b");
        assert_eq!(multi.text(), "a\nb");
        let (row, col) = multi.cursor_display();
        assert_eq!((row, col), (1, 1));
    }

    #[test]
    fn masked_display_hides_content() {
        let mut f = EditableField::new(false).masked(true);
        f.set_text("secret");
        assert_eq!(f.display_text(), "••••••");
        assert_eq!(f.text(), "secret");
    }

    #[test]
    fn history_recall_cycles() {
        let mut f = EditableField::new(false);
        f.push_history(":host 9000");
        f.push_history(":connect 1.2.3.4");
        f.set_text("draft");
        f.history_prev();
        assert_eq!(f.text(), ":connect 1.2.3.4");
        f.history_prev();
        assert_eq!(f.text(), ":host 9000");
        f.history_next();
        assert_eq!(f.text(), ":connect 1.2.3.4");
        f.history_next();
        assert_eq!(f.text(), "draft"); // stashed live text restored
    }
}
