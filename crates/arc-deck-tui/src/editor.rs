use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// An atomic piece of the draft: an `@path` reference or a paste summary. Backspace/Delete
/// remove the whole thing instead of one grapheme — upstream's `part.ts` behaves the same way,
/// and half-deleting a path is the kind of thing that only shows up when the prompt is sent.
///
/// `payload` is what the segment expands to on submit. File references have no payload: their
/// display text *is* the path. Paste summaries carry the original text here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Segment {
    /// Byte range in `Editor::text`.
    pub start: usize,
    pub end: usize,
    pub payload: Option<String>,
}

/// Byte cursor always sits at a grapheme boundary, including CJK and combining text.
#[derive(Debug, Default)]
pub(crate) struct Editor {
    pub text: String,
    pub cursor: usize,
    pub history: Vec<String>,
    segments: Vec<Segment>,
    history_index: Option<usize>,
    draft: String,
}

impl Editor {
    /// Terminal controls in a paste are data, never terminal commands.
    fn sanitize(text: &str) -> String {
        text.replace("\r\n", "\n")
            .replace('\r', "\n")
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect()
    }

    pub fn insert(&mut self, text: &str) {
        let text = Self::sanitize(text);
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.shift_segments(self.cursor - text.len(), text.len());
        // Inserting a combining mark can merge the adjacent graphemes.
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain([self.text.len()])
            .find(|i| *i >= self.cursor)
            .unwrap_or(self.text.len());
    }

    /// Inserts `display` as one indivisible segment. `payload` is the text it expands to when
    /// the draft is submitted (`None` means "the display text is the payload").
    pub fn insert_segment(&mut self, display: &str, payload: Option<&str>) {
        let display = Self::sanitize(display);
        let start = self.cursor;
        self.text.insert_str(start, &display);
        self.cursor += display.len();
        self.shift_segments(start, display.len());
        self.segments.push(Segment {
            start,
            end: self.cursor,
            payload: payload.map(Self::sanitize),
        });
    }

    /// Replaces `start..end` with plain text (accepting a slash command: the draft was exactly
    /// the command, so the whole thing is replaced).
    pub fn replace_range(&mut self, start: usize, end: usize, text: &str) {
        self.replace_inner(start, end, text, false);
    }

    /// Like `replace_range`, but the inserted text becomes one atomic segment — accepting an
    /// `@path` replaces the typed query with a reference that deletes as a unit.
    pub fn replace_with_segment(&mut self, start: usize, end: usize, text: &str, payload: Option<&str>) {
        self.replace_inner(start, end, text, true);
        if let Some(segment) = self.segments.last_mut() {
            segment.payload = payload.map(Self::sanitize);
        }
    }

    fn replace_inner(&mut self, start: usize, end: usize, text: &str, segment: bool) {
        let end = end.min(self.text.len());
        if start > end
            || !self.text.is_char_boundary(start)
            || !self.text.is_char_boundary(end)
        {
            return;
        }
        let text = Self::sanitize(text);
        // Segments fully inside the replaced range disappear with it; a segment the cursor is
        // mid-way through is kept by the clamp in `shift_segments_removed`.
        self.segments.retain(|item| item.end <= start || item.start >= end);
        self.text.replace_range(start..end, "");
        self.shift_segments_removed(start, end - start);
        self.text.insert_str(start, &text);
        self.shift_segments(start, text.len());
        self.cursor = start + text.len();
        if segment {
            self.segments.push(Segment {
                start,
                end: self.cursor,
                payload: None,
            });
        }
    }

    #[cfg(test)]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Replaces the whole draft (history recall, external editor). Segments do not survive:
    /// text that came from outside has no reference/paste structure.
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.cursor = self.text.len();
        self.segments.clear();
        self.history_index = None;
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
        // Inside (or at the end of) a segment, one Backspace removes the whole segment.
        if let Some(index) = self
            .segments
            .iter()
            .position(|s| s.start < self.cursor && self.cursor <= s.end)
        {
            self.remove_segment(index);
            return;
        }
        let end = self.cursor;
        self.left();
        self.text.replace_range(self.cursor..end, "");
        self.shift_segments_removed(self.cursor, end - self.cursor);
    }
    pub fn delete(&mut self) {
        if let Some(index) = self
            .segments
            .iter()
            .position(|s| s.start <= self.cursor && self.cursor < s.end)
        {
            self.remove_segment(index);
            return;
        }
        let end = self.cursor
            + self.text[self.cursor..]
                .graphemes(true)
                .next()
                .map(str::len)
                .unwrap_or(0);
        self.text.replace_range(self.cursor..end, "");
        self.shift_segments_removed(self.cursor, end - self.cursor);
    }
    fn remove_segment(&mut self, index: usize) {
        let segment = self.segments.remove(index);
        let len = segment.end - segment.start;
        self.text.replace_range(segment.start..segment.end, "");
        self.cursor = segment.start;
        self.shift_segments_removed(segment.start, len);
    }
    /// Moves segments that start after an insertion point. Segments never overlap an insertion
    /// made with the cursor at their edge (backspace/delete take the whole segment first).
    fn shift_segments(&mut self, at: usize, by: usize) {
        for segment in &mut self.segments {
            if segment.start >= at {
                segment.start += by;
                segment.end += by;
            }
        }
    }
    fn shift_segments_removed(&mut self, at: usize, removed: usize) {
        let end = at + removed;
        for segment in &mut self.segments {
            if segment.start >= end {
                segment.start -= removed;
                segment.end -= removed;
            } else if segment.end > at {
                // Should be unreachable for atomic edits, but a clamped segment is safer than
                // a panic or a stale range that would slice a UTF-8 character later.
                segment.start = segment.start.min(at);
                segment.end = segment.end.saturating_sub(removed).max(segment.start);
            }
        }
        self.segments.retain(|segment| segment.end > segment.start);
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
        let mut text = std::mem::take(&mut self.text);
        let mut segments = std::mem::take(&mut self.segments);
        segments.sort_by_key(|segment| segment.start);
        // Back to front: an expansion changes the length, and earlier offsets must stay valid.
        for segment in segments.iter().rev() {
            if let Some(payload) = &segment.payload
                && segment.end <= text.len()
            {
                text.replace_range(segment.start..segment.end, payload);
            }
        }
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
        // Recalled text has no reference/paste structure; keeping stale ranges would slice it.
        self.segments.clear();
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

    /// A file reference is one unit: Backspace at its end removes the whole thing, never half a
    /// path (a half path would be sent as a broken reference and look like a model problem).
    #[test]
    fn a_reference_segment_is_removed_whole() {
        let mut e = Editor::default();
        e.insert_segment("@src/main.rs", None);
        e.insert(" tail");
        for _ in 0..5 {
            e.left();
        }
        assert_eq!(e.cursor, "@src/main.rs".len());
        e.backspace();
        assert_eq!(e.text, " tail");
        assert!(e.segments().is_empty());

        let mut e = Editor::default();
        e.insert_segment("@x/y", None);
        e.insert("z");
        e.home();
        e.delete();
        assert_eq!(e.text, "z", "Delete at the segment start removes it whole");
    }

    /// Typing before a segment moves it, so its byte range keeps pointing at the same text.
    #[test]
    fn segments_shift_when_text_is_inserted_before_them() {
        let mut e = Editor::default();
        e.insert_segment("@a", None);
        e.home();
        e.insert("x");
        assert_eq!(e.text, "x@a");
        e.end();
        e.backspace();
        assert_eq!(e.text, "x");
    }

    /// A paste summary displays a placeholder and expands on submit; the draft itself never
    /// holds the whole paste twice.
    #[test]
    fn a_paste_summary_expands_when_submitted() {
        let mut e = Editor::default();
        e.insert("看：");
        e.insert_segment("[粘贴 3 行]", Some("一\n二\n三"));
        assert_eq!(e.text, "看：[粘贴 3 行]");
        assert_eq!(e.take(), "看：一\n二\n三");
        assert!(e.segments().is_empty());
        assert_eq!(*e.history.last().unwrap(), "看：一\n二\n三");
    }

    #[test]
    fn cjk_text_around_a_segment_keeps_the_cursor_on_boundaries() {
        let mut e = Editor::default();
        e.insert("中文");
        e.insert_segment("@文/件", None);
        assert_eq!(e.cursor, "中文@文/件".len());
        e.backspace();
        assert_eq!(e.text, "中文");
        assert_eq!(e.cursor, "中文".len());
        e.right();
        assert_eq!(e.cursor, "中文".len(), "right at the end is a no-op");
    }
}
