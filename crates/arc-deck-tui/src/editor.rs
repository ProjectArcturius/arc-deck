use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Byte cursor always sits at a grapheme boundary, including CJK and combining text.
#[derive(Debug, Default)]
pub(crate) struct Editor {
    pub text: String,
    pub cursor: usize,
    pub history: Vec<String>,
    history_index: Option<usize>,
    draft: String,
}

impl Editor {
    pub fn insert(&mut self, text: &str) {
        // Terminal controls in a paste are data, never terminal commands.
        let text: String = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        // Inserting a combining mark can merge the adjacent graphemes.
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain([self.text.len()])
            .find(|i| *i >= self.cursor)
            .unwrap_or(self.text.len());
    }
    pub fn left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }
    pub fn right(&mut self) {
        self.cursor += self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map(str::len)
            .unwrap_or(0);
    }
    pub fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.text.replace_range(self.cursor..end, "");
    }
    pub fn delete(&mut self) {
        let end = self.cursor
            + self.text[self.cursor..]
                .graphemes(true)
                .next()
                .map(str::len)
                .unwrap_or(0);
        self.text.replace_range(self.cursor..end, "");
    }
    pub fn home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
    }
    pub fn end(&mut self) {
        self.cursor += self.text[self.cursor..]
            .find('\n')
            .unwrap_or(self.text.len() - self.cursor);
    }
    pub fn vertical(&mut self, previous: bool) {
        let start = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let column = self.text[start..self.cursor].width();
        let target = if previous {
            if start == 0 {
                return;
            }
            let end = start - 1;
            let begin = self.text[..end].rfind('\n').map(|i| i + 1).unwrap_or(0);
            begin..end
        } else {
            let Some(n) = self.text[self.cursor..].find('\n') else {
                return;
            };
            let begin = self.cursor + n + 1;
            let end = begin
                + self.text[begin..]
                    .find('\n')
                    .unwrap_or(self.text.len() - begin);
            begin..end
        };
        let mut x = 0;
        self.cursor = target.start;
        for (offset, g) in self.text[target.clone()].grapheme_indices(true) {
            if x + g.width() > column {
                break;
            }
            self.cursor = target.start + offset + g.len();
            x += g.width();
        }
    }
    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.history_index = None;
        if !text.trim().is_empty() {
            self.history.push(text.clone());
        }
        text
    }
    pub fn history(&mut self, previous: bool) {
        if self.history.is_empty() {
            return;
        }
        let next = if previous {
            match self.history_index {
                None => {
                    self.draft = self.text.clone();
                    Some(self.history.len() - 1)
                }
                Some(i) => Some(i.saturating_sub(1)),
            }
        } else {
            self.history_index
                .and_then(|i| (i + 1 < self.history.len()).then_some(i + 1))
        };
        self.text = next
            .map(|i| self.history[i].clone())
            .unwrap_or_else(|| self.draft.clone());
        self.cursor = self.text.len();
        self.history_index = next;
    }
    pub fn wrapped(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(2);
        let mut rows = vec![String::new()];
        let mut x = 0;
        let mut cursor = (0, 0);
        for (offset, g) in self.text.grapheme_indices(true) {
            let glyph = if g == "\t" { "    " } else { g };
            let n = glyph.width();
            if g != "\n" && x + n > width {
                rows.push(String::new());
                x = 0;
            }
            if offset == self.cursor {
                cursor = (x, rows.len() - 1);
            }
            if g == "\n" {
                rows.push(String::new());
                x = 0;
            } else {
                rows.last_mut().unwrap().push_str(glyph);
                x += n;
            }
        }
        if self.cursor == self.text.len() {
            if x >= width {
                rows.push(String::new());
                x = 0;
            }
            cursor = (x, rows.len() - 1);
        }
        (rows, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cjk_emoji_combining_marks_survive_editing_and_wrapping() {
        let mut e = Editor::default();
        e.insert("中e\u{301}👩‍💻文");
        e.left();
        e.backspace();
        assert_eq!(e.text, "中e\u{301}文");
        e.backspace();
        assert_eq!(e.text, "中文");
        assert_eq!(e.wrapped(3), (vec!["中".into(), "文".into()], (0, 1)));
        e.delete();
        assert_eq!(e.text, "中");
    }
    #[test]
    fn paste_normalizes_newlines_and_does_not_submit() {
        let mut e = Editor::default();
        e.insert("你好\r\n第二行\u{1b}[31m");
        assert_eq!(e.text, "你好\n第二行[31m");
        assert!(e.history.is_empty());
    }
    #[test]
    fn history_restores_the_unsent_draft() {
        let mut e = Editor::default();
        e.insert("第一次");
        e.take();
        e.insert("草稿");
        e.history(true);
        assert_eq!(e.text, "第一次");
        e.history(false);
        assert_eq!(e.text, "草稿");
    }
    #[test]
    fn vertical_navigation_keeps_cell_columns_and_never_splits_a_grapheme() {
        let mut e = Editor::default();
        e.insert("中ab\n文cd");
        e.vertical(true);
        assert_eq!(e.cursor, "中ab".len());
        e.left();
        e.vertical(false);
        assert_eq!(e.cursor, "中ab\n文c".len());
    }
}
