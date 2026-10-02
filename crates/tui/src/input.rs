//! A one-line text field: the title filter, the `apply` confirmation and
//! the local-title editor. The keys are OpenTUI's `<input>`'s that the TS
//! TUI relies on: typing, backspace and delete, left and right, home and
//! end (`ctrl-a`, `ctrl-e`), and `ctrl-k`/`ctrl-u` to clear after or
//! before the cursor.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineInput {
    text: String,
    /// In characters.
    cursor: usize,
}

impl LineInput {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            cursor: text.chars().count(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text before the cursor, the character under it (if any), and
    /// the rest.
    pub fn split(&self) -> (&str, Option<char>, &str) {
        let at = self.byte_at(self.cursor);
        let under = self.text[at..].chars().next();
        let after = at + under.map_or(0, char::len_utf8);
        (&self.text[..at], under, &self.text[after..])
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(at, _)| at)
    }

    /// Apply an editing key; whether it changed the text.
    pub fn edit(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.text.chars().count();
        match key.code {
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = len,
            KeyCode::Char('k') if ctrl => {
                let at = self.byte_at(self.cursor);
                self.text.truncate(at);
                return true;
            }
            KeyCode::Char('u') if ctrl => {
                let at = self.byte_at(self.cursor);
                self.text.drain(..at);
                self.cursor = 0;
                return true;
            }
            KeyCode::Char(_) if ctrl => {}
            KeyCode::Char(c) => {
                let at = self.byte_at(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
                return true;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
                return true;
            }
            KeyCode::Delete if self.cursor < len => {
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
                return true;
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut input = LineInput::new("café");
        input.edit(key(KeyCode::Left));
        input.edit(key(KeyCode::Backspace));
        input.edit(key(KeyCode::Char('F')));
        assert_eq!(input.text(), "caFé");
        assert_eq!(input.split(), ("caF", Some('é'), ""));
        input.edit(ctrl('a'));
        input.edit(key(KeyCode::Delete));
        assert_eq!(input.text(), "aFé");
        input.edit(ctrl('k'));
        assert_eq!(input.text(), "");
        let mut input = LineInput::new("old title");
        input.edit(ctrl('u'));
        assert_eq!(input.text(), "");
    }
}
