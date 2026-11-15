//! A single-line text input: caret, selection, word jumps, clipboard.

#[derive(Default, Clone)]
pub struct TextBox {
    pub text: String,
    /// Byte offsets; the selection runs from `anchor` to `caret`.
    pub caret: usize,
    pub anchor: usize,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

impl TextBox {
    pub fn new(text: &str) -> TextBox {
        TextBox { text: text.to_string(), caret: text.len(), anchor: text.len() }
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.caret = text.len();
        self.anchor = text.len();
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    pub fn selected(&self) -> &str {
        let (a, b) = self.selection();
        &self.text[a..b]
    }

    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    fn delete_selection(&mut self) -> bool {
        if !self.has_selection() {
            return false;
        }
        let (a, b) = self.selection();
        self.text.replace_range(a..b, "");
        self.caret = a;
        self.anchor = a;
        true
    }

    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        // One line only: pasted newlines and tabs become spaces.
        let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        self.text.insert_str(self.caret, &s);
        self.caret += s.len();
        self.anchor = self.caret;
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map(|(j, _)| j).unwrap_or(0)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..].chars().next().map(|c| i + c.len_utf8()).unwrap_or(i)
    }

    fn word_left(&self, mut i: usize) -> usize {
        while i > 0 && !is_word(self.text[..i].chars().next_back().unwrap()) {
            i = self.prev(i);
        }
        while i > 0 && is_word(self.text[..i].chars().next_back().unwrap()) {
            i = self.prev(i);
        }
        i
    }

    fn word_right(&self, mut i: usize) -> usize {
        let n = self.text.len();
        while i < n && is_word(self.text[i..].chars().next().unwrap()) {
            i = self.next(i);
        }
        while i < n && !is_word(self.text[i..].chars().next().unwrap()) {
            i = self.next(i);
        }
        i
    }

    fn move_to(&mut self, i: usize, extend: bool) {
        self.caret = i;
        if !extend {
            self.anchor = i;
        }
    }

    pub fn left(&mut self, word: bool, extend: bool) {
        if !extend && self.has_selection() {
            let (a, _) = self.selection();
            return self.move_to(a, false);
        }
        let i = if word { self.word_left(self.caret) } else { self.prev(self.caret) };
        self.move_to(i, extend);
    }

    pub fn right(&mut self, word: bool, extend: bool) {
        if !extend && self.has_selection() {
            let (_, b) = self.selection();
            return self.move_to(b, false);
        }
        let i = if word { self.word_right(self.caret) } else { self.next(self.caret) };
        self.move_to(i, extend);
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn end(&mut self, extend: bool) {
        self.move_to(self.text.len(), extend);
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    pub fn backspace(&mut self, word: bool) {
        if self.delete_selection() {
            return;
        }
        let i = if word { self.word_left(self.caret) } else { self.prev(self.caret) };
        self.text.replace_range(i..self.caret, "");
        self.caret = i;
        self.anchor = i;
    }

    pub fn delete(&mut self, word: bool) {
        if self.delete_selection() {
            return;
        }
        let i = if word { self.word_right(self.caret) } else { self.next(self.caret) };
        self.text.replace_range(self.caret..i, "");
    }

    /// Cuts the selection, returning it.
    pub fn cut(&mut self) -> String {
        let s = self.selected().to_string();
        self.delete_selection();
        s
    }

    /// The caret's position counted in UTF-16 units, for DirectWrite hit tests.
    pub fn utf16_pos(&self, byte: usize) -> u32 {
        self.text[..byte].encode_utf16().count() as u32
    }

    /// The byte offset of a UTF-16 position.
    pub fn byte_pos(&self, u16pos: u32) -> usize {
        let mut n = 0u32;
        for (i, c) in self.text.char_indices() {
            if n >= u16pos {
                return i;
            }
            n += c.len_utf16() as u32;
        }
        self.text.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing() {
        let mut t = TextBox::new("excel | code");
        t.backspace(true);
        assert_eq!(t.text, "excel | ");
        t.backspace(true);
        assert_eq!(t.text, "");
        t.insert("abc");
        t.left(false, true);
        assert_eq!(t.selected(), "c");
        t.insert("é");
        assert_eq!(t.text, "abé");
        t.left(false, false);
        t.delete(false);
        assert_eq!(t.text, "ab");
        assert_eq!(t.byte_pos(t.utf16_pos(2)), 2);
    }
}
