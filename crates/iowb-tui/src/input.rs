//! The prompt box: text with a cursor that moves the way it does in an editor.

use unicode_width::UnicodeWidthChar;

/// Text and a cursor, kept on a character boundary.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Input {
    text: String,
    cursor: usize,
}

/// The text cut into rows of a given width, and where the cursor is in them.
pub struct Layout {
    /// Each row's first byte in the text, and its text.
    pub rows: Vec<(usize, String)>,
    pub cursor_row: usize,
    pub cursor_col: usize,
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

impl Input {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Replaces the text and puts the cursor after it.
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn insert(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn before(&self) -> &str {
        &self.text[..self.cursor]
    }

    fn after(&self) -> &str {
        &self.text[self.cursor..]
    }

    pub fn left(&mut self) {
        if let Some(ch) = self.before().chars().next_back() {
            self.cursor -= ch.len_utf8();
        }
    }

    pub fn right(&mut self) {
        if let Some(ch) = self.after().chars().next() {
            self.cursor += ch.len_utf8();
        }
    }

    /// Where the previous word starts.
    fn word_start(&self) -> usize {
        let mut at = self.cursor;
        let mut chars = self.before().chars().rev().peekable();
        while let Some(ch) = chars.next_if(|ch| !is_word(*ch)) {
            at -= ch.len_utf8();
        }
        while let Some(ch) = chars.next_if(|ch| is_word(*ch)) {
            at -= ch.len_utf8();
        }
        at
    }

    /// Where the next word ends.
    fn word_end(&self) -> usize {
        let mut at = self.cursor;
        let mut chars = self.after().chars().peekable();
        while let Some(ch) = chars.next_if(|ch| !is_word(*ch)) {
            at += ch.len_utf8();
        }
        while let Some(ch) = chars.next_if(|ch| is_word(*ch)) {
            at += ch.len_utf8();
        }
        at
    }

    pub fn word_left(&mut self) {
        self.cursor = self.word_start();
    }

    pub fn word_right(&mut self) {
        self.cursor = self.word_end();
    }

    fn remove(&mut self, from: usize, to: usize) {
        self.text.replace_range(from..to, "");
        self.cursor = from;
    }

    pub fn backspace(&mut self) {
        let from = self.cursor - self.before().chars().next_back().map_or(0, char::len_utf8);
        self.remove(from, self.cursor);
    }

    pub fn delete(&mut self) {
        let to = self.cursor + self.after().chars().next().map_or(0, char::len_utf8);
        self.remove(self.cursor, to);
    }

    pub fn delete_word_left(&mut self) {
        self.remove(self.word_start(), self.cursor);
    }

    pub fn delete_word_right(&mut self) {
        self.remove(self.cursor, self.word_end());
    }

    /// The start of the line the cursor is on (lines end at a newline, not at a wrap).
    fn line_start_offset(&self) -> usize {
        self.before().rfind('\n').map_or(0, |newline| newline + 1)
    }

    fn line_end_offset(&self) -> usize {
        self.after()
            .find('\n')
            .map_or(self.text.len(), |newline| self.cursor + newline)
    }

    pub fn line_start(&mut self) {
        self.cursor = self.line_start_offset();
    }

    pub fn line_end(&mut self) {
        self.cursor = self.line_end_offset();
    }

    /// Cuts the text into rows of at most `width` columns. A wide character is
    /// never split, and a cursor after a full last row gets a row of its own.
    pub fn layout(&self, width: usize) -> Layout {
        let width = width.max(1);
        let mut rows = vec![(0, String::new())];
        let mut col = 0;
        let mut cursor = None;
        for (offset, ch) in self.text.char_indices() {
            if ch == '\n' {
                if offset == self.cursor {
                    cursor = Some((rows.len() - 1, col));
                }
                rows.push((offset + 1, String::new()));
                col = 0;
                continue;
            }
            let w = ch.width().unwrap_or(0);
            if col + w > width && col > 0 {
                rows.push((offset, String::new()));
                col = 0;
            }
            if offset == self.cursor {
                cursor = Some((rows.len() - 1, col));
            }
            if let Some((_, row)) = rows.last_mut() {
                row.push(ch);
            }
            col += w;
        }
        let (cursor_row, cursor_col) = cursor.unwrap_or_else(|| {
            if col >= width {
                rows.push((self.text.len(), String::new()));
                (rows.len() - 1, 0)
            } else {
                (rows.len() - 1, col)
            }
        });
        Layout {
            rows,
            cursor_row,
            cursor_col,
        }
    }

    /// Moves the cursor one row up, keeping its column. False when it is already on
    /// the first row, so the caller can use the key for something else.
    pub fn up(&mut self, width: usize) -> bool {
        self.step_row(width, -1)
    }

    pub fn down(&mut self, width: usize) -> bool {
        self.step_row(width, 1)
    }

    fn step_row(&mut self, width: usize, delta: isize) -> bool {
        let layout = self.layout(width);
        let target = layout.cursor_row as isize + delta;
        if target < 0 || target as usize >= layout.rows.len() {
            return false;
        }
        let (start, row) = &layout.rows[target as usize];
        let mut used = 0;
        let mut offset = *start + row.len();
        for (index, ch) in row.char_indices() {
            let w = ch.width().unwrap_or(0);
            if used + w > layout.cursor_col {
                offset = *start + index;
                break;
            }
            used += w;
        }
        self.cursor = offset;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Input {
        let mut input = Input::default();
        input.insert(text);
        input
    }

    /// The text with `|` where the cursor is.
    fn shown(input: &Input) -> String {
        format!("{}|{}", input.before(), input.after())
    }

    #[test]
    fn typing_inserts_at_the_cursor_not_at_the_end() {
        let mut input = typed("hello world");
        input.left();
        input.left();
        input.left();
        input.left();
        input.left();
        input.left();
        input.insert(",");
        assert_eq!(shown(&input), "hello,| world");
    }

    #[test]
    fn left_and_right_stop_at_the_ends_and_cross_multibyte_characters() {
        let mut input = typed("a日é");
        input.right();
        assert_eq!(shown(&input), "a日é|");
        for _ in 0..5 {
            input.left();
        }
        assert_eq!(shown(&input), "|a日é");
        input.right();
        input.right();
        assert_eq!(shown(&input), "a日|é");
    }

    #[test]
    fn backspace_and_delete_remove_whole_characters() {
        let mut input = typed("a日é");
        input.left();
        input.backspace();
        assert_eq!(shown(&input), "a|é");
        input.delete();
        assert_eq!(shown(&input), "a|");
        input.delete();
        input.backspace();
        input.backspace();
        input.backspace();
        assert_eq!(shown(&input), "|");
    }

    #[test]
    fn word_moves_jump_over_words_and_punctuation() {
        let mut input = typed("fn main() { let x = 1; }");
        input.word_left();
        assert_eq!(shown(&input), "fn main() { let x = |1; }");
        input.word_left();
        assert_eq!(shown(&input), "fn main() { let |x = 1; }");
        input.word_right();
        assert_eq!(shown(&input), "fn main() { let x| = 1; }");
        for _ in 0..10 {
            input.word_left();
        }
        assert_eq!(shown(&input), "|fn main() { let x = 1; }");
        input.word_right();
        assert_eq!(shown(&input), "fn| main() { let x = 1; }");
    }

    #[test]
    fn deleting_words_in_both_directions() {
        let mut input = typed("one two  three");
        input.delete_word_left();
        assert_eq!(shown(&input), "one two  |");
        input.delete_word_left();
        assert_eq!(shown(&input), "one |");
        let mut middle = typed("one two three");
        middle.word_left();
        middle.word_left();
        assert_eq!(shown(&middle), "one |two three");
        middle.delete_word_right();
        assert_eq!(shown(&middle), "one | three");
    }

    #[test]
    fn home_and_end_work_on_the_current_line_of_several() {
        let mut input = typed("first\nsecond\nthird");
        input.up(80);
        input.line_end();
        assert_eq!(shown(&input), "first\nsecond|\nthird");
        input.line_start();
        assert_eq!(shown(&input), "first\n|second\nthird");
    }

    #[test]
    fn up_and_down_move_between_rows_keeping_the_column() {
        let mut input = typed("abcdef\nxy\nghijkl");
        // The cursor is at the end of "ghijkl", column 6.
        assert!(input.up(80));
        assert_eq!(
            shown(&input),
            "abcdef\nxy|\nghijkl",
            "the short row ends before the column"
        );
        assert!(input.up(80));
        assert_eq!(
            shown(&input),
            "ab|cdef\nxy\nghijkl",
            "the column it has now, 2"
        );
        assert!(!input.up(80), "nothing above the first row");
        assert!(input.down(80));
        assert!(input.down(80));
        assert!(!input.down(80), "nothing below the last row");
    }

    #[test]
    fn a_long_line_is_cut_into_rows_and_up_and_down_follow_them() {
        let mut input = typed("abcdefghij");
        let layout = input.layout(4);
        let rows: Vec<&str> = layout.rows.iter().map(|(_, row)| row.as_str()).collect();
        assert_eq!(rows, ["abcd", "efgh", "ij"]);
        assert_eq!((layout.cursor_row, layout.cursor_col), (2, 2));

        assert!(input.up(4));
        assert_eq!(shown(&input), "abcdef|ghij");
        assert!(input.up(4));
        assert_eq!(shown(&input), "ab|cdefghij");
        assert!(input.down(4));
        assert_eq!(shown(&input), "abcdef|ghij");
    }

    #[test]
    fn the_cursor_after_a_full_last_row_gets_a_row_of_its_own() {
        let layout = typed("abcd").layout(4);
        assert_eq!(layout.rows.len(), 2);
        assert_eq!((layout.cursor_row, layout.cursor_col), (1, 0));

        let mut full = typed("abcd");
        full.left();
        let layout = full.layout(4);
        assert_eq!((layout.cursor_row, layout.cursor_col), (0, 3));
    }

    #[test]
    fn wide_characters_are_not_split_across_rows() {
        let layout = typed("日本語").layout(5);
        let rows: Vec<&str> = layout.rows.iter().map(|(_, row)| row.as_str()).collect();
        assert_eq!(rows, ["日本", "語"]);
        assert_eq!((layout.cursor_row, layout.cursor_col), (1, 2));
    }

    #[test]
    fn an_empty_input_and_a_trailing_newline_have_a_cursor_row() {
        let layout = Input::default().layout(10);
        assert_eq!(
            (layout.rows.len(), layout.cursor_row, layout.cursor_col),
            (1, 0, 0)
        );
        let layout = typed("a\n").layout(10);
        assert_eq!(
            (layout.rows.len(), layout.cursor_row, layout.cursor_col),
            (2, 1, 0)
        );
    }

    #[test]
    fn set_and_clear_reset_the_cursor() {
        let mut input = typed("old");
        input.left();
        input.set("brand new");
        assert_eq!(shown(&input), "brand new|");
        input.clear();
        assert_eq!(shown(&input), "|");
        assert!(input.is_empty());
    }
}
