//! Assistant Markdown for the transcript body (PRODUCT_PLAN §7.3「消息」).
//!
//! Structure and interaction follow OpenCode v1.19.0; the markdown token names
//! (`markdownText` / `markdownHeading` / `markdownLink` / …) are the ones that
//! version defines in `packages/tui/src/theme/index.ts`, so the palette here is
//! its direct counterpart. Copyright (c) 2025 opencode. MIT notice: ../LICENSE-OpenCode.
//!
//! Why the palette is a parameter instead of a `Theme` read: the theme system is
//! being rewritten by another worker in the same milestone, and a renderer that
//! reaches into `Theme` would have to change with it. Everything this module needs
//! is a colour, so a colour is what the caller passes.
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Colours the markdown renderer needs.
///
/// Modifiers are not in the palette: `*斜*` meaning italic is syntax, not theme, so
/// it is fixed here. Deviation from v1.19.0 worth recording: that version also
/// underlines level-1 headings (`markup.heading.1` in `theme/index.ts`), which in a
/// terminal is the same signal it uses for links — all six levels are bold + heading
/// colour here.
#[derive(Debug, Clone)]
pub(crate) struct MarkdownPalette {
    pub text: Color,
    pub muted: Color,
    pub heading: Color,
    /// `markdownLink`. Nothing in this mode consumes it: the task fixes the printed
    /// shape of a link as `文字 (url)` with the URL in `muted`, so the colour tree-sitter
    /// gives `markup.link.url` in v1.19.0's `theme/index.ts` — `markdownLink`, underlined
    /// — is the one part of this palette no branch reaches. Recorded as a deviation
    /// rather than quietly switching the URL to it: "dimmer than the text" is what makes
    /// a long URL recede, and a "show syntax" toggle would be this field's first real
    /// user. (`allow(dead_code)` is for that reservation, not to hide a leak.)
    #[allow(dead_code)]
    pub link: Color,
    pub link_text: Color,
    /// Inline code only. Fenced/indented code uses `code_block_fg` / `code_block_bg`.
    pub code: Color,
    pub code_block_fg: Color,
    pub code_block_bg: Color,
    pub block_quote: Color,
    pub emph: Color,
    pub strong: Color,
    pub list_marker: Color,
    pub hr: Color,
}

/// Renders `text` into lines already wrapped to `width` terminal cells.
///
/// Markdown syntax is concealed rather than printed. The citation for that choice is
/// OpenCode v1.19.0 `packages/tui/src/routes/session/index.tsx`, which renders
/// assistant text through `TextPart`: it holds `const [conceal, setConceal] =
/// createSignal(true)`, binds that signal to the `session.toggle.conceal` keybind,
/// and passes `conceal={ctx.conceal()}` to `<markdown>`. So the reference UI starts
/// with the markers hidden and lets the user toggle them — the default here matches,
/// and there is no toggle yet (see the palette's note on `link`).
///
/// How far that concealment reaches is **not** verified. `refs/opencode` has no
/// `node_modules`, so OpenTUI's `conceal` prop cannot be read; its keybind is labelled
/// "Toggle code concealment", which suggests it may only cover code fences. This
/// renderer conceals heading, emphasis, code and link syntax alike, because the task
/// this module implements asks for the markers to be hidden (`#` explicitly) and a
/// half-concealed document reads worse than either extreme. Recorded as a deviation,
/// not as OpenTUI's behaviour.
pub(crate) fn render(text: &str, width: u16, palette: &MarkdownPalette) -> Vec<Line<'static>> {
    let text = clean(text);
    if text.trim().is_empty() {
        // An empty document still yields one blank line from pulldown-cmark, which
        // would show as a stray gap wherever a message has no body yet.
        return Vec::new();
    }
    let width = (width as usize).max(2);
    Writer::new(width, palette.clone()).run(&text)
}

/// Drops control characters and keeps newlines.
///
/// A tab is dropped rather than expanded to a tab stop: tab width belongs to the
/// terminal, and guessing one here would put the wrapping and the terminal out of
/// step. (`view.rs::clean` does the same to every other field it prints.)
pub(crate) fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}

/// Code blocks are indented three cells, matching the `<box paddingLeft={3}>` that
/// wraps a message part in v1.19.0's `TextPart`.
const CODE_INDENT: usize = 3;
/// One list nesting level; v1.19.0 indents list content by two cells.
const LIST_STEP: usize = 2;

/// One list nesting level. `indent` is the column its marker starts at.
struct ItemState {
    ordered: bool,
    next: u64,
    indent: usize,
    /// Set by `Event::TaskListMarker`, which arrives right after the item starts.
    task: Option<bool>,
    /// This item's marker has been written for the paragraph being flushed; the
    /// rest of that paragraph's rows get the shadow prefix instead. Kept per item
    /// rather than per writer so a nested list inside an item still gets its own
    /// marker.
    marked: bool,
}

impl ItemState {
    /// Cells the marker occupies, including the space before the content.
    fn width(&self) -> usize {
        let marker = if self.ordered {
            self.next.to_string().len()
        } else {
            1
        };
        // "[x] " / "[ ] " is four cells, between the marker and the content.
        marker + 1 + if self.task.is_some() { 4 } else { 0 }
    }
}

enum Frame {
    Item(ItemState),
    Quote,
}

/// Cells an output line spends before its content starts.
#[derive(Clone, Copy)]
enum Indent {
    /// List level with its marker: the marker and the space after it.
    Item(usize),
    /// Block quote bars, or an empty frame.
    Spaces(usize),
}

impl Indent {
    fn width(self) -> usize {
        match self {
            Indent::Item(width) | Indent::Spaces(width) => width,
        }
    }
}

/// A table cell. `open` flips to false at `End(TableCell)`, which is how
/// `Event::Text` tells "append to this cell" from "this row is finished".
struct Cell {
    text: String,
    open: bool,
}

struct TableState {
    align: Vec<Alignment>,
    rows: Vec<Vec<Cell>>,
    row: Vec<Cell>,
    header_rows: usize,
}

struct CodeState {
    label: Option<String>,
    lines: Vec<String>,
    line: String,
}

/// A single wrapped row: the graphemes of one output line, ready to be given a
/// block prefix.
type Row = Vec<(String, Style, bool)>;

struct Writer {
    width: usize,
    palette: MarkdownPalette,
    out: Vec<Line<'static>>,
    /// Spans of the current logical line, before wrapping.
    spans: Vec<Span<'static>>,
    block: Vec<Frame>,
    styles: Vec<Style>,
    code: Option<CodeState>,
    table: Option<TableState>,
    in_table_cell: bool,
    /// Link text accumulated by `push`, and the destination of the same link. Both
    /// are stacks only because the event stream is a tree walk; Markdown cannot
    /// actually nest one link in another.
    links: Vec<String>,
    link_urls: Vec<String>,
    images: Vec<String>,
    /// A blank line was asked for; it is emitted just before the next content line.
    blank_wanted: bool,
    /// Something block-level has been emitted; guards the leading blank rule.
    content: bool,
}

impl Writer {
    fn new(width: usize, palette: MarkdownPalette) -> Self {
        Self {
            width,
            palette,
            out: Vec::new(),
            spans: Vec::new(),
            block: Vec::new(),
            styles: Vec::new(),
            code: None,
            table: None,
            in_table_cell: false,
            links: Vec::new(),
            link_urls: Vec::new(),
            images: Vec::new(),
            blank_wanted: false,
            content: false,
        }
    }

    fn run(mut self, text: &str) -> Vec<Line<'static>> {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_STRIKETHROUGH);
        options.insert(Options::ENABLE_TASKLISTS);
        // ENABLE_GFM is deliberately left off: in 0.13.4 it only adds blockquote tags
        // ([!NOTE] …), and the transcript has no room for a tag line above every quote.
        for event in Parser::new_ext(text, options) {
            match event {
                Event::Start(tag) => self.start(tag),
                Event::End(tag) => self.end(tag),
                Event::Text(value) => {
                    let style = self.inline_style();
                    self.text_styled(&value, style);
                }
                Event::Code(value) => {
                    let style = self.inline_code_style();
                    self.push(value.to_string(), style);
                }
                Event::SoftBreak => {
                    let style = self.inline_style();
                    self.push(" ".to_string(), style);
                }
                Event::HardBreak => self.flush(),
                Event::Rule => self.rule(),
                Event::TaskListMarker(checked) => {
                    if let Some(Frame::Item(item)) = self.block.last_mut() {
                        item.task = Some(checked);
                    }
                }
                Event::Html(value) | Event::InlineHtml(value) => {
                    let style = self.muted_style();
                    self.text_styled(&value, style);
                }
                Event::FootnoteReference(label) => {
                    let style = self.muted_style();
                    self.push(format!("[{label}]"), style);
                }
                // Math is not enabled, so these cannot arrive today; the TeX source as
                // muted text is the honest fallback if that ever changes.
                Event::InlineMath(value) | Event::DisplayMath(value) => {
                    let style = self.muted_style();
                    self.push(value.to_string(), style);
                }
            }
        }
        self.finish();
        self.out
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                // A paragraph directly inside a list item is the item's own content; a
                // blank line there would double-space every tight list.
                if !matches!(self.block.last(), Some(Frame::Item(_))) {
                    self.blank_wanted = true;
                }
            }
            Tag::Heading { .. } => {
                // Heading spacing rule: one blank line above when anything precedes it,
                // one blank line below. A heading at the very start of a message
                // therefore gets no leading blank, and never two.
                self.blank_wanted = true;
                // Bold + heading colour, and the `#` markers never enter the buffer:
                // that *is* the concealment, and it belongs to the renderer rather than
                // to the text, so nothing downstream has to strip markers again.
                let style = self
                    .inline_style()
                    .fg(self.palette.heading)
                    .add_modifier(Modifier::BOLD);
                self.styles.push(style);
            }
            Tag::BlockQuote(_) => {
                if !matches!(self.block.last(), Some(Frame::Item(_))) {
                    self.blank_wanted = true;
                } else {
                    // A quote is a block *inside* the item, not more item text, so it
                    // starts on its own row. `End(Paragraph)` flushes the item's
                    // paragraph too late to help here: a tight list has no `Paragraph`
                    // at all, and the `Start(BlockQuote)` arrives with the item's text
                    // still in the buffer (measured: `• │ 父引用 续`, four blocks on one
                    // row). Flushing here is what splits them.
                    self.flush();
                }
                self.block.push(Frame::Quote);
            }
            Tag::CodeBlock(kind) => {
                self.blank_wanted = true;
                self.code = Some(CodeState {
                    label: match &kind {
                        // The info string may carry more than the language; its first
                        // word is the label, as in CommonMark's own examples.
                        CodeBlockKind::Fenced(info) => info
                            .split_whitespace()
                            .next()
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                        CodeBlockKind::Indented => None,
                    },
                    lines: Vec::new(),
                    line: String::new(),
                });
            }
            Tag::List(start) => {
                // Nested lists indent by depth; the outermost starts at column 0 so a
                // top-level list is not pushed right for no reason.
                let nested = self.block.iter().any(|f| matches!(f, Frame::Item(_)));
                // Measured, not counted from the frame stack: `block.len() * LIST_STEP`
                // treated every open frame as a list level, but a quote is a frame too.
                // A list inside a quote then started at six cells instead of two, and
                // its nested child landed ten cells in against an eight cell terminal
                // (`│         • 子`). A list level starts `LIST_STEP` past its parent's
                // *content*, which is the only thing that keeps children inside parents.
                let indent = if nested {
                    self.child_indent()
                } else {
                    0
                };
                if !nested {
                    self.blank_wanted = true;
                } else {
                    // A nested list starts its own line. A tight list emits no
                    // `Paragraph` events (measured: `Start(Item) Text(…) Start(List)`
                    // with nothing in between), so without this flush the child item's
                    // text was appended to the parent item's line.
                    self.flush();
                }
                self.block.push(Frame::Item(ItemState {
                    ordered: start.is_some(),
                    next: start.unwrap_or(1),
                    indent,
                    task: None,
                    marked: false,
                }));
            }
            Tag::Item => {
                // This item is a fresh marker, and a blank line the enclosing list
                // asked for must not leak into the item's own paragraph — a tight list
                // would then get a gap between every pair of items.
                self.blank_wanted = false;
                if let Some(Frame::Item(item)) = self.block.last_mut() {
                    item.task = None;
                    item.marked = false;
                }
            }
            Tag::Table(align) => {
                self.blank_wanted = true;
                self.table = Some(TableState {
                    align,
                    rows: Vec::new(),
                    row: Vec::new(),
                    header_rows: 0,
                });
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.header_rows += 1;
                }
            }
            Tag::TableRow => {}
            Tag::TableCell => {
                self.in_table_cell = true;
                if let Some(t) = self.table.as_mut() {
                    t.row.push(Cell {
                        text: String::new(),
                        open: true,
                    });
                }
            }
            Tag::Emphasis => {
                let style = self.inline_style().fg(self.palette.emph);
                self.styles.push(style.add_modifier(Modifier::ITALIC));
            }
            Tag::Strong => {
                let style = self.inline_style().fg(self.palette.strong);
                self.styles.push(style.add_modifier(Modifier::BOLD));
            }
            Tag::Strikethrough => {
                let style = self.inline_style();
                self.styles.push(style.add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { dest_url, .. } => {
                // The text of a link carries `link_text`; the URL appended on close is
                // painted separately in `muted`. `palette.link` itself is for the
                // concealed `[]()` syntax (see the field's note).
                let style = self.inline_style().fg(self.palette.link_text);
                self.styles.push(style);
                self.links.push(String::new());
                self.link_urls.push(dest_url.to_string());
            }
            Tag::Image { .. } => self.images.push(String::new()),
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_)
            // Superscript/subscript need their own options, which are off, so these
            // cannot arrive; they would render as plain text if they did.
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                // A paragraph is followed by a blank before the next block. This has to
                // be asked for here rather than only at the next block's `Start`, because
                // `Event::Rule` has no `Start` at all — without it a rule ran straight
                // into the paragraph above it (measured: `["a", "────────"]`).
                self.blank_wanted = true;
            }
            TagEnd::Heading(_) => {
                self.styles.pop();
                self.flush();
                self.blank_wanted = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.block.pop();
                // A quote is followed by a blank, like every other block. Without this
                // the paragraph *after* the quote was appended to the quote's line
                // (measured: `• │ 父引用 续`, where the quote and the item's own text
                // were one row). Asking for the blank and letting the next block spend it
                // also keeps a quote at the end of a message from leaving a blank behind,
                // because nothing follows to spend it on.
                self.blank_wanted = true;
            }
            TagEnd::CodeBlock => self.end_code(),
            TagEnd::List(_) => {
                self.flush();
                self.block.pop();
                if !self.block.iter().any(|f| matches!(f, Frame::Item(_))) {
                    self.blank_wanted = true;
                }
            }
            TagEnd::Item => {
                self.flush();
                if let Some(Frame::Item(item)) = self.block.last_mut()
                    && item.ordered
                {
                    item.next += 1;
                }
            }
            TagEnd::Table => self.end_table(),
            TagEnd::TableHead => self.close_row(),
            TagEnd::TableRow => self.close_row(),
            TagEnd::TableCell => {
                self.in_table_cell = false;
                if let Some(t) = self.table.as_mut()
                    && let Some(cell) = t.row.last_mut()
                {
                    cell.open = false;
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                let label = self.links.pop().unwrap_or_default();
                let destination = self.link_urls.pop().unwrap_or_default();
                self.finish_link(&label, &destination);
            }
            TagEnd::Image => {
                let alt = self.images.pop().unwrap_or_default();
                if let Some(outer) = self.images.last_mut() {
                    outer.push_str(&alt);
                } else {
                    let label = if alt.trim().is_empty() {
                        "[图]".to_string()
                    } else {
                        format!("[图: {alt}]")
                    };
                    let style = self.muted_style();
                    self.push(label, style);
                }
            }
            _ => {}
        }
    }

    /// Link text in `link_text` colour, and — only when the two differ — the
    /// destination appended in muted colour as `(url)`.
    ///
    /// Printing the URL unconditionally would duplicate every autolink
    /// (`<https://x>` has the URL as its own text), which is the case the
    /// "only when they differ" rule exists for.
    fn finish_link(&mut self, label: &str, destination: &str) {
        let label = label.trim();
        if label.is_empty() {
            let style = self.muted_style();
            self.push(destination.trim().to_string(), style);
            return;
        }
        if label != destination.trim() {
            let style = self.muted_style();
            self.push(format!(" ({})", destination.trim()), style);
        }
    }

    fn end_code(&mut self) {
        let Some(mut state) = self.code.take() else {
            return;
        };
        if !state.line.is_empty() {
            state.lines.push(std::mem::take(&mut state.line));
        }
        while state.lines.last().is_some_and(|l| l.trim().is_empty()) {
            state.lines.pop();
        }
        if state.lines.is_empty() && state.label.is_none() {
            // An empty fence separates nothing, so it must not leave a blank behind.
            return;
        }
        // The blank asked for at `Start(CodeBlock)` belongs *above* the block, so it is
        // written here (after the empty-block check) rather than left for the next
        // `flush_pending`, which would drop it for having nothing to flush.
        if self.blank_wanted && self.content && !self.out.is_empty() {
            self.out.push(Line::default());
        }
        self.blank_wanted = false;
        self.content = true;
        // A code block nested in a quote (or a list item) is drawn with the same block
        // prefix the prose gets, so the nesting survives into the code. Without it the
        // quote bar simply vanished and the block floated out to the left margin
        // (measured: `   code` inside `> ```` `> code` ````).
        let prefix = self.prefix(self.content_indent(), false);
        let lead = prefix_width(&prefix);
        if let Some(label) = state.label.clone() {
            let style = self.muted_style().add_modifier(Modifier::DIM);
            self.out.push(prefix_line(prefix.clone(), Line::styled(label, style)));
        }
        // The `…` needs one cell of its own, hence the two-cell floor: at a width that
        // leaves less than that there is no honest way to show any of the line.
        let content = self.width.saturating_sub(lead + CODE_INDENT).max(2);
        for line in &state.lines {
            // Truncate, never wrap: a wrapped code line reports a line break the code
            // does not have. The trailing `…` is the one honest signal that what is on
            // screen is not the whole line.
            let shown = truncate_to_width(line, content);
            let used = display_width(&shown);
            // Painted to the full width, so the block reads as one panel instead of a
            // column of ragged highlights. Widening the code indent never widens this
            // padding: it is the slack left over, never negative.
            let pad = " ".repeat(self.width.saturating_sub(lead + CODE_INDENT + used));
            let style = self.code_style();
            let mut spans = prefix.clone();
            spans.push(Span::raw(" ".repeat(CODE_INDENT)));
            spans.push(Span::styled(shown, style));
            spans.push(Span::styled(pad, style));
            self.out.push(Line::from(spans));
        }
        self.blank_wanted = true;
    }

    fn end_table(&mut self) {
        let Some(mut table) = self.table.take() else {
            return;
        };
        if !table.row.is_empty() {
            table.rows.push(std::mem::take(&mut table.row));
        }
        self.flush_pending();
        self.content = true;
        self.blank_wanted = true;
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let rows: Vec<Vec<String>> = table
            .rows
            .iter()
            .map(|row| row.iter().map(|c| c.text.clone()).collect())
            .collect();
        let indent = self.content_indent();
        let avail = self.width.saturating_sub(indent.width());
        match render_table(&rows, &table.align, table.header_rows, avail, &self.palette) {
            Some(rendered) => {
                for line in rendered {
                    let prefix = self.prefix(indent, false);
                    self.out.push(prefix_line(prefix, line));
                }
            }
            None => {
                // Degradation rule, the one this renderer commits to and the one the
                // test pins: a grid costs `3 × 列数 - 1` cells of overhead before a
                // single character of content. If even one cell per column does not
                // fit, the grid cannot be drawn at all, so each row is printed as its
                // cells joined by ` · ` and wrapped like a paragraph. Truncating the
                // columns instead would drop right-hand values without saying so, and a
                // table is not code — a wrap does not change its meaning.
                for row in &rows {
                    let joined = row
                        .iter()
                        .map(|c| c.trim())
                        .filter(|c| !c.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    if joined.is_empty() {
                        continue;
                    }
                    let style = self.text_style();
                    self.wrap_line(vec![Span::styled(joined, style)]);
                }
            }
        }
    }

    /// Flushes the current logical line, wrapped, honouring a pending blank line.
    fn flush(&mut self) {
        self.flush_pending();
    }

    /// Emits the pending blank line (if any) before the next block's content, then
    /// the current line. One helper for both, so the "blank above/below" rule cannot
    /// drift between block kinds.
    ///
    /// Nothing is flushed when there is no line, and the pending blank is **kept**.
    /// Clearing it there threw the blank away instead of spending it: `End(Paragraph)`
    /// asks for a blank, `Start(Rule)` calls this with no line pending, and the rule
    /// then followed the paragraph with nothing between them (measured).
    fn flush_pending(&mut self) {
        let spans = std::mem::take(&mut self.spans);
        if spans.is_empty() {
            return;
        }
        if self.blank_wanted && self.content && !self.out.is_empty() {
            self.out.push(Line::default());
        }
        self.blank_wanted = false;
        self.wrap_line(spans);
        self.content = true;
    }

    fn rule(&mut self) {
        self.flush_pending();
        // A rule carries a blank line above and below, like every other block. Asking
        // for the blank above with `blank_wanted` did **not** work: `flush_pending`
        // bails out when there is no line pending and leaves the flag set, but by then
        // the paragraph above has already been flushed *and* has already cleared the
        // flag — so the rule went straight under it (measured: `["a", "────────"]`).
        // Flushing the blank on its own is unambiguous because a blank line is never
        // the last thing a document wants (`finish` trims trailing blanks anyway).
        if self.content && !self.out.is_empty() {
            self.out.push(Line::default());
        }
        self.content = true;
        self.blank_wanted = true;
        let indent = self.content_indent();
        let count = self.width.saturating_sub(indent.width()).max(1);
        let style = Style::default().fg(self.palette.hr);
        // A rule is never wrapped: it is a divider, drawn to the width it has.
        let prefix = self.prefix(indent, false);
        self.out.push(prefix_line(prefix, Line::styled("─".repeat(count), style)));
    }

    /// Appends verbatim content. A newline inside the event is a real line break for
    /// code (the event is a block) and for raw HTML, which is shown as plain text.
    fn text_styled(&mut self, value: &str, style: Style) {
        if self.table.is_some() {
            self.fill_table_cell(value);
            return;
        }
        if let Some(state) = self.code.as_mut() {
            let mut rest = value;
            while let Some(index) = rest.find('\n') {
                state.line.push_str(&rest[..index]);
                state.lines.push(std::mem::take(&mut state.line));
                rest = &rest[index + 1..];
            }
            state.line.push_str(rest);
            return;
        }
        let mut rest = value;
        while let Some(index) = rest.find('\n') {
            let (head, tail) = rest.split_at(index);
            if !head.is_empty() {
                self.push(head.to_string(), style);
            }
            self.flush();
            rest = &tail[1..];
        }
        if !rest.is_empty() {
            self.push(rest.to_string(), style);
        }
    }

    fn fill_table_cell(&mut self, value: &str) {
        if !self.in_table_cell {
            return;
        }
        if let Some(state) = self.table.as_mut()
            && let Some(cell) = state.row.last_mut()
            && cell.open
        {
            cell.text.push_str(value);
        }
    }

    fn close_row(&mut self) {
        if let Some(t) = self.table.as_mut()
            && !t.row.is_empty()
        {
            t.rows.push(std::mem::take(&mut t.row));
        }
    }

    /// Appends a span.
    ///
    /// Inside a link or image the text is only *collected*: a link's text is already
    /// on the line because its own spans were pushed as it arrived, and an image's alt
    /// text must not be shown at all — the whole image becomes `[图: alt]` on close.
    /// Collecting and also pushing is what printed an image's alt text twice
    /// (`架构图[图: 架构图]`), so the two cases are separated here.
    fn push(&mut self, value: String, style: Style) {
        if let Some(state) = self.images.last_mut() {
            state.push_str(&value);
            return;
        }
        if let Some(state) = self.links.last_mut() {
            state.push_str(&value);
        }
        self.spans.push(Span::styled(value, style));
    }

    fn text_style(&self) -> Style {
        Style::default().fg(self.palette.text)
    }

    fn muted_style(&self) -> Style {
        Style::default().fg(self.palette.muted)
    }

    fn code_style(&self) -> Style {
        Style::default()
            .fg(self.palette.code_block_fg)
            .bg(self.palette.code_block_bg)
    }

    fn inline_code_style(&self) -> Style {
        Style::default()
            .fg(self.palette.code)
            .bg(self.palette.code_block_bg)
    }

    fn inline_style(&self) -> Style {
        self.styles
            .last()
            .copied()
            .unwrap_or_else(|| self.text_style())
    }

    /// Cells an output line spends before its content starts, given the open blocks.
    /// The innermost list level's own `indent` already accounts for the levels above
    /// it, so this is that level plus its marker plus any quote bars.
    fn content_indent(&self) -> Indent {
        let quote = self
            .block
            .iter()
            .filter(|f| matches!(f, Frame::Quote))
            .count()
            * (CODE_INDENT - 1);
        let item = self
            .block
            .iter()
            .filter_map(|f| match f {
                Frame::Item(item) => Some(item),
                Frame::Quote => None,
            })
            .next_back();
        match item {
            Some(item) => Indent::Item(item.indent + item.width() + quote),
            None => Indent::Spaces(quote),
        }
    }

    /// Column a list nested one level deeper than the innermost open item starts at —
    /// `LIST_STEP` past the level above it, which is how the project's nested lists
    /// have always been laid out (and what `nested_list_continuation_hangs_under_nested_content`
    /// pins).
    ///
    /// Derived from the innermost `Item`, never from the frame stack: a quote is a
    /// frame but not a level, so `block.len() * LIST_STEP` counted it and a list inside
    /// a quote started six cells in instead of two. Measured at width 8, where the
    /// quote's own list then overflowed the terminal (`│         • 子`).
    ///
    /// A list that opens directly inside a quote with no item above it starts at the
    /// quote's content, which is where `content_indent` puts the quote's prose.
    fn child_indent(&self) -> usize {
        match self.block.last() {
            Some(Frame::Item(item)) => item.indent + LIST_STEP,
            Some(Frame::Quote) => CODE_INDENT - 1,
            None => 0,
        }
    }

    /// Leading spans of an output line: quote bars, then the marker of one list level
    /// (the others contribute only indentation, which is what makes a nested list sit
    /// inside its parent and a wrapped row hang under its own text).
    ///
    /// `indent` decides which level owns the marker. While a block is being written
    /// that level is the innermost one; once the block is closed and later blocks are
    /// emitted (a heading, a rule, a table), the outermost level owns it because the
    /// inner list is no longer the thing being drawn. `continuation` says this row is
    /// a wrap of the row above it, which never gets the marker again.
    ///
    /// Every level *above* the marker contributes the width of one content start and
    /// nothing else. Padding to the marker level by the full item width instead pushed
    /// a nested marker four cells past its parent's text (measured: `    • 父子`
    /// where `  • 子` was expected).
    fn prefix(&self, indent: Indent, continuation: bool) -> Vec<Span<'static>> {
        let marker_level = match indent {
            Indent::Item(_) => self.innermost_item(),
            Indent::Spaces(_) => self.outermost_item(),
        };
        let starts = self.item_starts();
        let quote_style = Style::default().fg(self.palette.block_quote);
        let marker_style = Style::default().fg(self.palette.list_marker);
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (index, frame) in self.block.iter().enumerate() {
            match frame {
                // The bar is on every line of the quote, continuation included.
                Frame::Quote => spans.push(Span::styled("│ ", quote_style)),
                Frame::Item(item) => {
                    if Some(index) != marker_level {
                        // Levels above the marker only push the line right to where the
                        // marker level starts — marker width included, because the
                        // parent's own marker (and its task box) sits in that space.
                        spans.push(Span::raw(" ".repeat(starts.get(index).copied().unwrap_or(0))));
                        continue;
                    }
                    let start = starts.get(index).copied().unwrap_or(item.indent);
                    let marker_here = !item.marked && !continuation;
                    if !marker_here {
                        // Continuation of the same paragraph: same columns, no glyphs.
                        spans.push(Span::raw(" ".repeat(start + item.width())));
                        continue;
                    }
                    spans.push(Span::raw(" ".repeat(item.indent)));
                    if item.ordered {
                        spans.push(Span::styled(format!("{}. ", item.next), marker_style));
                    } else {
                        spans.push(Span::styled("• ", marker_style));
                    }
                    if let Some(checked) = item.task {
                        let box_ = if checked { "[x] " } else { "[ ] " };
                        spans.push(Span::styled(box_, marker_style));
                    }
                }
            }
        }
        spans
    }

    /// Column at which each open list level's first line starts, given the levels
    /// above it. Each level's start is its own `indent` — the value `child_indent` gave
    /// it when it opened — and what a *child* adds on top is its marker width, which is
    /// what makes a nested list sit inside its parent rather than at the same column.
    ///
    /// The `start` values here and `indent` are therefore the same numbers by
    /// construction, which is what lets `prefix` subtract one from the other and land a
    /// marker exactly on its column.
    fn item_starts(&self) -> Vec<usize> {
        self.block
            .iter()
            .filter_map(|f| match f {
                Frame::Item(item) => Some(item.indent),
                Frame::Quote => None,
            })
            .collect()
    }

    fn innermost_item(&self) -> Option<usize> {
        self.block
            .iter()
            .enumerate()
            .filter(|(_, f)| matches!(f, Frame::Item(_)))
            .map(|(index, _)| index)
            .next_back()
    }

    fn outermost_item(&self) -> Option<usize> {
        self.block
            .iter()
            .position(|f| matches!(f, Frame::Item(_)))
    }

    /// Word-wraps one logical line into `out`. A term longer than the width is cut at
    /// the width; otherwise the break goes at the last space that fits. No grapheme is
    /// ever split, and the spaces a break lands on are dropped.
    fn wrap_line(&mut self, spans: Vec<Span<'static>>) {
        let mut atoms: Vec<(String, Style, bool)> = Vec::new();
        for span in &spans {
            for grapheme in span.content.graphemes(true) {
                atoms.push((
                    grapheme.to_string(),
                    span.style,
                    grapheme.chars().all(char::is_whitespace),
                ));
            }
        }
        // The prefix is charged on the first row and the hanging indent on the rest,
        // so the text wrapped here is the text that actually fits after the prefix.
        let indent = self.content_indent();
        let first = self.width.saturating_sub(indent.width()).max(1);
        let rest = self.width.saturating_sub(indent.width()).max(1);
        let mut rows: Vec<Row> = Vec::new();
        let mut row: Row = Vec::new();
        let mut used = 0;
        let mut limit = first;
        for atom in atoms {
            let cells = display_width(&atom.0);
            if used + cells > limit && !row.is_empty() {
                let at = row.iter().rposition(|a| a.2).map(|i| i + 1);
                let carried = row.split_off(at.unwrap_or(row.len()));
                rows.push(std::mem::take(&mut row));
                row = carried;
                // Leading spaces of the new row are the ones the break landed on.
                while row.first().is_some_and(|a| a.2) {
                    row.remove(0);
                }
                // From here on this is a continuation row: hanging indent, not marker.
                limit = rest;
                used = row.iter().map(|a| display_width(&a.0)).sum();
            }
            used += cells;
            row.push(atom);
        }
        rows.push(row);
        let level = match indent {
            Indent::Item(_) => self.innermost_item(),
            Indent::Spaces(_) => self.outermost_item(),
        };
        for (index, row) in rows.iter().enumerate() {
            self.emit(row, indent, index > 0);
        }
        // The marker is spent once the first row is on the page; rows of the *next*
        // paragraph in the same item (and this paragraph's wrapped rows, through the
        // `continuation` flag) get the shadow prefix instead.
        if let Some(level) = level
            && let Some(Frame::Item(item)) = self.block.get_mut(level)
        {
            item.marked = true;
        }
    }

    /// Turns one wrapped row into a `Line`, adding the block prefix or its shadow.
    fn emit(&mut self, row: &Row, indent: Indent, continuation: bool) {
        if row.is_empty() {
            return;
        }
        // Trailing spaces a break landed on are dropped; interior and leading spaces
        // are content and stay.
        let end = row
            .iter()
            .rposition(|a| !a.2)
            .map(|i| i + 1)
            .unwrap_or(row.len());
        let row = &row[..end];
        if row.is_empty() {
            return;
        }
        let mut spans = self.prefix(indent, continuation);
        // Nothing fitted after the prefix, so the wrap kept one atom per row and the
        // finished row came out wider than the terminal (measured: a `• [ ] ` task
        // prefix is six cells, and at width 6 every row was eight cells wide — the
        // prefix plus the one cell `wrap_line` always allows by flooring its limit at
        // one). Charge the prefix against the budget here as well, so the width the
        // caller asked for is a property of the output rather than of the wrap alone.
        let budget = self.width.saturating_sub(prefix_width(&spans));
        let mut run = String::new();
        let mut style: Option<Style> = None;
        let mut used = 0;
        for (text, atom_style, _) in row {
            let cells = display_width(text);
            if used + cells > budget {
                break;
            }
            if style.is_some_and(|s| s != *atom_style) {
                spans.push(Span::styled(std::mem::take(&mut run), style.unwrap()));
            }
            style = Some(*atom_style);
            used += cells;
            run.push_str(text);
        }
        if let Some(style) = style {
            spans.push(Span::styled(run, style));
        }
        self.out.push(Line::from(spans));
    }

    fn finish(&mut self) {
        self.flush_pending();
        while self.out.last().is_some_and(|l| l.width() == 0) {
            self.out.pop();
        }
    }
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Cells a block prefix occupies. Measured from the spans themselves rather than
/// recomputed from the frame, because `prefix` decides which level owns the marker and
/// the two must not drift.
fn prefix_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|s| display_width(&s.content)).sum()
}

/// Cuts `text` to `width` cells and marks the cut with `…`. Never splits a grapheme.
fn truncate_to_width(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    // One cell is given up for the ellipsis so the result still fits.
    let budget = width.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let cells = display_width(grapheme);
        if used + cells > budget {
            break;
        }
        out.push_str(grapheme);
        used += cells;
    }
    out.push('…');
    out
}

fn prefix_line(prefix: Vec<Span<'static>>, line: Line<'static>) -> Line<'static> {
    let mut spans = prefix;
    spans.extend(line.spans);
    Line::from(spans).style(line.style)
}

/// Lays out a grid from already-collected rows. `None` means "does not fit"; the
/// caller owns the degradation rule (see `Writer::end_table`).
fn render_table(
    rows: &[Vec<String>],
    align: &[Alignment],
    header_rows: usize,
    avail: usize,
    palette: &MarkdownPalette,
) -> Option<Vec<Line<'static>>> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return None;
    }
    // `│` between columns, one space of padding on each side of every column.
    let overhead = 3 * columns - 1;
    if avail < overhead + columns {
        return None;
    }
    let mut widths = vec![1usize; columns];
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < columns {
                widths[index] = widths[index].max(display_width(cell.trim()));
            }
        }
    }
    let mut total: usize = widths.iter().sum();
    while total + overhead > avail {
        // Out of room: shrink the widest column first, so a table of two long cells
        // degrades evenly instead of losing its tail. Never below one cell per column
        // — `avail >= overhead + columns` above guarantees this loop can stop.
        let Some(index) = (0..columns)
            .filter(|i| widths[*i] > 1)
            .max_by_key(|i| widths[*i])
        else {
            break;
        };
        widths[index] -= 1;
        total -= 1;
    }
    let border = Style::default().fg(palette.muted);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if index == header_rows && rows.len() > header_rows {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (column, width) in widths.iter().enumerate() {
                if column > 0 {
                    spans.push(Span::styled("┼", border));
                }
                spans.push(Span::styled("─".repeat(width + 2), border));
            }
            lines.push(Line::from(spans));
        }
        let style = if index < header_rows {
            Style::default()
                .fg(palette.text)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.text)
        };
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (column, width) in widths.iter().enumerate() {
            if column > 0 {
                spans.push(Span::styled("│", border));
            }
            let cell = row.get(column).map(String::as_str).unwrap_or("");
            // Cells are clipped with the same rule as code lines, so a long cell never
            // pushes the grid past the terminal.
            let cell = truncate_to_width(cell.trim(), *width);
            let pad = width.saturating_sub(display_width(&cell));
            let (left, right) = match align.get(column).copied().unwrap_or(Alignment::None) {
                Alignment::Right => (pad, 0),
                Alignment::Center => (pad / 2, pad - pad / 2),
                Alignment::Left | Alignment::None => (0, pad),
            };
            spans.push(Span::styled(
                format!(" {}{cell}{} ", " ".repeat(left), " ".repeat(right)),
                style,
            ));
        }
        lines.push(Line::from(spans));
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn palette() -> MarkdownPalette {
        let t = Theme::new(false);
        MarkdownPalette {
            text: t.text,
            muted: t.text_muted,
            heading: t.action,
            link: t.action,
            link_text: t.action_selected,
            code: t.feedback_success,
            code_block_fg: t.text,
            code_block_bg: t.background_element,
            block_quote: t.action,
            emph: t.action_selected,
            strong: t.text,
            list_marker: t.action_disabled,
            hr: t.border,
        }
    }

    fn lines(source: &str, width: u16) -> Vec<Line<'static>> {
        render(source, width, &palette())
    }

    fn plain(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    fn texts(source: &str, width: u16) -> Vec<String> {
        lines(source, width).iter().map(plain).collect()
    }

    fn span_of(line: &Line<'static>, text: &str) -> Style {
        line.spans
            .iter()
            .find(|s| s.content.contains(text))
            .unwrap_or_else(|| panic!("no span containing {text:?} in {line:?}"))
            .style
    }

    #[test]
    fn empty_and_whitespace_only_input_render_nothing() {
        assert!(lines("", 40).is_empty());
        assert!(lines("   \n\n  \t \n", 40).is_empty());
    }

    #[test]
    fn heading_conceals_hashes_and_is_bold() {
        let rendered = lines("# 标题\n\n正文", 40);
        assert_eq!(texts("# 标题\n\n正文", 40), vec!["标题", "", "正文"]);
        assert_eq!(
            span_of(&rendered[0], "标题"),
            Style::default()
                .fg(palette().heading)
                .add_modifier(Modifier::BOLD)
        );
        assert_eq!(
            span_of(&rendered[2], "正文"),
            Style::default().fg(palette().text)
        );
        // One blank above, one below, and no leading blank at the start of a message.
        assert_eq!(
            texts("# 一\n\n二\n\n## 三", 40),
            vec!["一", "", "二", "", "三"]
        );
        assert_eq!(texts("## 三", 40), vec!["三"]);
    }

    #[test]
    fn emphasis_strong_and_strikethrough_set_modifiers_and_colors() {
        let rendered = lines("*斜* **粗** ~~删~~", 40);
        assert_eq!(rendered.len(), 1);
        assert_eq!(plain(&rendered[0]), "斜 粗 删");
        assert_eq!(
            span_of(&rendered[0], "斜"),
            Style::default()
                .fg(palette().emph)
                .add_modifier(Modifier::ITALIC)
        );
        assert_eq!(
            span_of(&rendered[0], "粗"),
            Style::default()
                .fg(palette().strong)
                .add_modifier(Modifier::BOLD)
        );
        assert_eq!(
            span_of(&rendered[0], "删"),
            Style::default()
                .fg(palette().text)
                .add_modifier(Modifier::CROSSED_OUT)
        );
    }

    #[test]
    fn inline_code_keeps_stars_as_text_and_drops_backticks() {
        let rendered = lines("使用 `a * b` 连接", 40);
        assert_eq!(plain(&rendered[0]), "使用 a * b 连接");
        assert_eq!(
            span_of(&rendered[0], "a * b"),
            Style::default()
                .fg(palette().code)
                .bg(palette().code_block_bg)
        );
        // The `*` inside the code span did not open an emphasis run.
        assert!(
            rendered[0]
                .spans
                .iter()
                .all(|s| !s.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn soft_break_joins_and_hard_break_splits() {
        assert_eq!(texts("一\n二", 40), vec!["一 二"]);
        assert_eq!(texts("一  \n二", 40), vec!["一", "二"]);
        assert_eq!(texts("一\\\n二", 40), vec!["一", "二"]);
    }

    #[test]
    fn chinese_wraps_by_terminal_cells_not_characters() {
        // Six CJK glyphs are twelve cells, so width 10 takes five (ten cells) and
        // carries the sixth. There is no space to break at, so the cut is at the limit.
        assert_eq!(texts("中文中文中文", 10), vec!["中文中文中", "文"]);
        for line in lines("中文中文中文", 10) {
            assert!(line.width() <= 10, "{line:?}");
        }
        // Latin text breaks at a word boundary when one fits.
        assert_eq!(texts("alpha beta gamma", 12), vec!["alpha beta", "gamma"]);
        // A word longer than the width is cut; no grapheme is split, so the combining
        // accent stays attached to its base letter.
        assert_eq!(texts("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(
            texts("e\u{301}e\u{301}e\u{301}", 2),
            vec!["e\u{301}e\u{301}", "e\u{301}"]
        );
    }

    #[test]
    fn unordered_list_marker_is_colored_and_continuation_is_hanging() {
        let rendered = lines(
            "- 第一项内容较长需要折行处理看看缩进对不对\n- 第二项",
            20,
        );
        let content = rendered.iter().map(plain).collect::<Vec<_>>();
        // The item is 21 cells of bullet + text, so it takes two rows at width 20.
        assert_eq!(content.len(), 4, "{content:?}");
        assert!(content[0].starts_with("• 第一项"), "{content:?}");
        assert!(content[1].starts_with("  "), "{content:?}");
        assert!(!content[1].contains('•'), "{content:?}");
        assert!(content[3].starts_with("• 第二项"), "{content:?}");
        assert_eq!(
            span_of(&rendered[0], "•"),
            Style::default().fg(palette().list_marker)
        );
        for line in &rendered {
            assert!(line.width() <= 20, "{line:?}");
        }
    }

    #[test]
    fn ordered_nested_and_task_lists_keep_their_own_markers() {
        let source = "1. 一\n2. 二\n\n- 父\n  - 子\n- [x] 完成\n- [ ] 未完成";
        let content = texts(source, 40);
        assert!(content.iter().any(|l| l.starts_with("1. 一")), "{content:?}");
        assert!(content.iter().any(|l| l.starts_with("2. 二")), "{content:?}");
        assert!(content.iter().any(|l| l.starts_with("• 父")), "{content:?}");
        let nested = content.iter().find(|l| l.contains('子')).unwrap();
        assert_eq!(nested, "  • 子");
        assert!(content.iter().any(|l| l == "• [x] 完成"), "{content:?}");
        assert!(content.iter().any(|l| l == "• [ ] 未完成"), "{content:?}");
    }

    #[test]
    fn nested_list_continuation_hangs_under_nested_content() {
        let content = texts(
            "- 父项很长很长很长很长很长很长\n  - 子项也很长很长很长很长很长\n",
            16,
        );
        assert!(content[0].starts_with("• 父项"), "{content:?}");
        let child = content
            .iter()
            .position(|l| l.trim_start().starts_with("• 子项"))
            .expect("child marker");
        assert!(content[child].starts_with("  • 子项"), "{content:?}");
        // The child's continuation aligns under the child's content, four cells in.
        let next = &content[child + 1];
        assert!(next.starts_with("    "), "{content:?}");
        assert!(!next.contains('•'), "{content:?}");
    }

    #[test]
    fn block_quote_draws_a_bar_on_every_line_of_a_wrapped_quote() {
        let rendered = lines("> 引用内容很长需要折行\n> 第二行", 12);
        let content = rendered.iter().map(plain).collect::<Vec<_>>();
        assert!(content.len() >= 3, "{content:?}");
        for line in &content {
            assert!(line.starts_with("│ "), "{line:?}");
        }
        assert_eq!(
            span_of(&rendered[0], "│"),
            Style::default().fg(palette().block_quote)
        );
        for line in &rendered {
            assert!(line.width() <= 12, "{line:?}");
        }
        // Quotes nest, and each level contributes its own bar.
        let nested = texts("> > 深处", 20);
        assert!(nested[0].starts_with("│ │ "), "{nested:?}");
    }

    #[test]
    fn code_block_truncates_instead_of_wrapping_and_shows_the_language() {
        let source = "```rust\nlet x = 1; // 这一行会超过宽度限制被截断\n# 不是标题\n```";
        let rendered = lines(source, 24);
        let content = rendered.iter().map(plain).collect::<Vec<_>>();
        assert_eq!(content.len(), 3, "{content:?}");
        assert_eq!(content[0], "rust");
        assert_eq!(content[1].width(), 24);
        assert!(content[1].ends_with('…'), "{content:?}");
        // `#` inside a fence is code, never a heading.
        assert!(content[2].contains("# 不是标题"), "{content:?}");
        assert_eq!(
            span_of(&rendered[1], "let x"),
            Style::default()
                .fg(palette().code_block_fg)
                .bg(palette().code_block_bg)
        );
        // The padding carries the background too, so the block is a full-width panel.
        assert_eq!(
            rendered[1].spans.last().unwrap().style.bg,
            Some(palette().code_block_bg)
        );
    }

    #[test]
    fn indented_code_block_needs_no_language_label() {
        let content = texts("文字\n\n    indented_code()\n", 40);
        assert_eq!(content.len(), 3, "{content:?}");
        assert_eq!(content[0], "文字");
        // A block is separated from the prose above it by one blank line.
        assert_eq!(content[1], "");
        // The trailing spaces are the block's background padding, not content.
        assert_eq!(content[2], format!("{:width$}", "   indented_code()", width = 40));
    }

    #[test]
    fn link_shows_text_and_appends_the_url_only_when_they_differ() {
        let rendered = lines("[文档](https://example.com/x)", 40);
        assert_eq!(plain(&rendered[0]), "文档 (https://example.com/x)");
        assert_eq!(
            span_of(&rendered[0], "文档"),
            Style::default().fg(palette().link_text)
        );
        assert_eq!(
            span_of(&rendered[0], "(https"),
            Style::default().fg(palette().muted)
        );
        // An autolink's text already is the URL, so appending it would print twice.
        assert_eq!(
            texts("<https://example.com/x>", 40),
            vec!["https://example.com/x"]
        );
        assert_eq!(
            texts("[https://e.com](https://e.com)", 40),
            vec!["https://e.com"]
        );
    }

    #[test]
    fn image_renders_as_placeholder_text() {
        assert_eq!(texts("![架构图](a.png)", 40), vec!["[图: 架构图]"]);
        assert_eq!(texts("![](a.png)", 40), vec!["[图]"]);
    }

    #[test]
    fn table_aligns_columns_by_display_width() {
        let source = "| 名称 | 值 |\n|:---|---:|\n| a | 中文 |\n| 长长 | b |";
        let content = texts(source, 40);
        assert_eq!(content.len(), 4, "{content:?}");
        // The `│` and `┼` land on the same column, and the grid width is measured in
        // terminal cells — `中文` is four cells, so the second column is four wide.
        let bar = content[2].chars().position(|c| c == '│').expect("bar");
        let cross = content[1].chars().position(|c| c == '┼').expect("cross");
        assert_eq!(bar, 6, "{content:?}");
        assert_eq!(cross, 6, "{content:?}");
        for (row, line) in content.iter().enumerate() {
            assert_eq!(
                display_width(line),
                display_width(&content[2]),
                "row {row}: {line:?}"
            );
        }
        // Column 1 is right-aligned (`---:`), so its value is padded on the left.
        assert_eq!(content[3], " 长长 │    b ");
        // Column 0 is left-aligned, so `a` is padded out to the column width.
        assert_eq!(content[2], " a    │ 中文 ");
    }

    #[test]
    fn table_wider_than_the_terminal_degrades_to_lines() {
        // Three columns need 3 × 3 − 1 = 8 cells of grid before any content, so at
        // width 10 no grid can be drawn and the rows degrade to plain lines.
        let source = "| 名称 | 值 | 备注 |\n| --- | --- | --- |\n| a | 短 | b |";
        let content = texts(source, 10);
        assert!(content.iter().any(|l| l.contains(" · ")), "{content:?}");
        assert!(!content.iter().any(|l| l.contains('│')), "{content:?}");
        for line in lines(source, 10) {
            assert!(line.width() <= 10, "{line:?}");
        }
    }

    #[test]
    fn rule_is_a_full_width_line_of_hr_color() {
        let rendered = lines("a\n\n---\n\nb", 8);
        let content = rendered.iter().map(plain).collect::<Vec<_>>();
        // One blank line above and below, like every other block, and the rule spans
        // the full width it was given.
        assert_eq!(content, vec!["a", "", "────────", "", "b"]);
        let rule = rendered
            .iter()
            .position(|l| plain(l).starts_with('─'))
            .expect("rule line");
        assert_eq!(rendered[rule].style.fg, Some(palette().hr));
        // At the very start of a message there is nothing to separate it from.
        assert_eq!(texts("---\n\nb", 8), vec!["────────", "", "b"]);
    }

    /// Every construct in one document, so the width sweep below covers the whole
    /// renderer rather than one shape of line.
    const EVERYTHING: &str = "# 标题\n\n\
        段落里有 *强调*、**加粗**、~~删除线~~、`行内代码` 与 [链接](https://example.com/very/long/url)。\n\n\
        > 引用块里有中文需要折行\n\
        > > 嵌套引用\n\n\
        - 无序项很长很长很长需要折行处理\n\
          - 嵌套子项也很长很长很长需要折行\n\
        - [ ] 未完成任务项很长很长\n\
        - [x] 已完成\n\n\
        1. 有序项\n\
        2. 第二项\n\n\
        - 列表项里的引用：\n\n\
          > 引用在列表项里\n\n\
        ```rust\n\
        let x = 1; // 这一行足够长，用来验证代码块是截断而不是折行\n\
        # 代码里的井号不是标题\n\
        ```\n\n\
        | 名称 | 值 |\n|:---|---:|\n| 中文 | a |\n\n\
        ---\n\n\
        <div>原始 HTML</div>\n\
        \n\
        > ```\n> 引用里的代码块\n> ```\n";

    #[test]
    fn no_width_can_produce_an_over_wide_line() {
        // Measured across every width, so the guarantee is not three hand-picked ones.
        // The renderer floors its own width at two cells (`render`), and a CJK glyph is
        // two cells, so below that nothing can be drawn at all.
        //
        // Widths below six are excluded because a block prefix can itself be wider than
        // the terminal there: each quote level costs `│ ` (two cells) and a task item
        // costs `• [ ] ` (six). Two nested quotes at width 2 have nothing left to
        // truncate — rendering the prefix and dropping the text is the least-bad answer
        // (`emit` does exactly that), and collapsing the prefix would silently drop the
        // nesting itself. The TUI refuses to draw below 20 columns (`view::draw`), so
        // that window is unreachable in practice.
        for width in 6u16..=48 {
            for line in lines(EVERYTHING, width) {
                assert!(
                    line.width() <= width as usize,
                    "width {width} produced a {} cell line: {line:?}",
                    line.width()
                );
            }
        }
    }

    #[test]
    fn a_prefix_wider_than_the_terminal_truncates_instead_of_overflowing() {
        // A `• [ ] ` prefix is six cells. At width 6 `wrap_line` has no room left for
        // content and floors its limit at one cell, so every row came out six cells of
        // prefix plus one of text — eight cells wide against a six cell terminal
        // (measured before the budget check in `emit`). The line must give way, not
        // the terminal.
        let rendered = lines("- [ ] 任务\n- [x] 完成", 6);
        assert!(!rendered.is_empty());
        for line in &rendered {
            assert!(line.width() <= 6, "{line:?}");
        }
        // The marker survives, so the line still reads as a task item.
        assert!(plain(&rendered[0]).starts_with("• [ ]"), "{rendered:?}");
    }

    #[test]
    fn nested_blocks_keep_their_nesting() {
        // A list inside a quote is one level deep, not two: a quote is a frame but not
        // a list level, so counting frames put the child at column ten against an eight
        // cell terminal (`│         • 子`). Now it is the quote bar plus one `LIST_STEP`.
        let quoted = texts("> - 父\n>   - 子", 20);
        assert_eq!(quoted, vec!["│ • 父", "│     • 子"]);
        for line in lines("> - 父\n>   - 子", 8) {
            assert!(line.width() <= 8, "{line:?}");
        }
        // A quote inside an item is a block of its own, not more item text. A tight
        // list emits no `Paragraph` for the item, so the item's text was still buffered
        // when the quote opened and all four blocks landed on one row
        // (measured: `• │ 父引用 续`). The child's bar sits past the item's marker.
        assert_eq!(
            texts("- 父\n  > 引用\n  > 续", 20),
            vec!["• 父", "", "  │ 引用 续"]
        );
        // A code block inside a quote keeps the bar. It is written straight to `out`
        // rather than through `wrap_line`, so it used to float out to the left margin
        // with the nesting silently gone (measured: `   code`).
        let code = texts("> ```\n> code\n> ```", 20);
        assert_eq!(code.len(), 1);
        assert!(code[0].starts_with("│    code"), "{code:?}");
        assert_eq!(lines("> ```\n> code\n> ```", 20)[0].width(), 20);
    }

    #[test]
    fn raw_html_is_shown_as_muted_plain_text() {
        let rendered = lines("<div>\nhello\n</div>", 40);
        let content = rendered.iter().map(plain).collect::<Vec<_>>();
        assert!(content.iter().any(|l| l.contains("<div>")), "{content:?}");
        assert!(content.iter().any(|l| l.contains("hello")), "{content:?}");
        for span in rendered.iter().flat_map(|l| &l.spans) {
            assert_eq!(span.style.fg, Some(palette().muted), "{:?}", span.content);
        }
        // Inline HTML stays literal text instead of being parsed.
        assert!(texts("前 <b>粗</b> 后", 40)[0].contains("<b>"));
    }

    #[test]
    fn control_characters_never_reach_the_transcript() {
        assert_eq!(clean("a\u{1b}[31mb\rc"), "a[31mbc");
    }

    #[test]
    fn every_line_respects_the_requested_width() {
        let source = "# 标题\n\n> 引用里的一段中文需要折行\n\n- 列表项也好长好长好长\n\n```\ncode_with_a_very_long_line()\n```";
        // Kept alongside the sweep above because it pins the three widths the TUI
        // actually draws at (`view::draw` refuses to go below 20 columns), where the
        // sweep covers the whole range including widths no user can reach.
        for width in [8u16, 16, 40] {
            for line in lines(source, width) {
                assert!(line.width() <= width as usize, "{width}: {line:?}");
            }
        }
    }
}
