# 任务书 w1-markdown：助手正文的 Markdown 渲染

（先读同目录 `_common.md` 的通用约定，它的全部条款适用于本任务。路径：
`C:/Users/mc158/Documents/Projects/arc-deck/tasks/m4/_common.md`）

## 背景

`crates/arc-deck-tui/src/view.rs` 里的 `fn markdown` 只认 fenced code 与 `#` 开头的行，其余原样输出。
PRODUCT_PLAN §7.3「消息」要求助手 Markdown/代码正确呈现。依赖 `pulldown-cmark` 已经加好。

## 要做的

1. 新建 `crates/arc-deck-tui/src/markdown.rs`：
   - `pub(crate) struct MarkdownPalette`：本模块需要的全部颜色（至少：text、muted、heading、link、link_text、
     code（行内）、code_block_fg、code_block_bg、block_quote、emph、strong、list_marker、hr）。
     **调色板由调用方传入**，模块自己不读 `Theme` —— 另一个工人正在重写主题系统，解耦才不会冲突。
   - `pub(crate) fn render(text: &str, width: u16, palette: &MarkdownPalette) -> Vec<Line<'static>>`：
     按给定宽度输出已折行的行。
2. 支持（用 pulldown-cmark，开 tables / strikethrough / tasklists 选项）：
   - 标题：隐藏 `#` 标记（OpenCode 默认 conceal Markdown 语法，去 v1.19.0 的 `routes/session` 里核对
     `conceal` 的用法并在注释里注明出处），加粗 + heading 色；标题前后留空行的规则写清楚。
   - 段落、软/硬换行；**强调 / 加粗 / 删除线**；行内代码（code 色，不加反引号）。
   - 无序 / 有序 / 嵌套列表：标记用 list_marker 色；**折行后续行悬挂缩进**对齐到正文起点；任务列表 `[ ]` / `[x]`。
   - 引用块：左侧竖线 + block_quote 色，可嵌套，折行后续行也带竖线。
   - fenced / 缩进代码块：code_block_bg 背景，**不折行而是按宽度截断并在行尾标 `…`**（代码折行会改变语义）；
     第一行之上显示语言标签（有的话）。**不做语法高亮**（写进最终回复的未做项）。
   - 链接：链接文字 link_text 色；URL 与文字不同时在后面以 muted 色附 `(url)`。图片显示 `[图: alt]`。
   - 表格：按列宽对齐（CJK 按终端格宽算，用 unicode-width）；总宽超出时做合理降级并在注释里写明规则。
   - 分隔线：整行 hr 色的 `─`。
   - 原始 HTML：按纯文本 muted 显示，不解析。
3. 折行：按**终端格宽**（CJK 占 2 格）在词边界折，长于一行的单词硬切；不得把一个字素切开。
4. 在 `view.rs` 里把旧 `fn markdown` 的调用换成新模块：用当前 `Theme` 的字段拼出 `MarkdownPalette`
   （写一个小的 `fn markdown_palette(t: &Theme) -> MarkdownPalette`，挑最接近的现有颜色）。
   保留 `clean()` 对控制字符的清理。正文已经有外层 `wrap()` 的地方，确认不会被二次折行打乱缩进。
5. 测试（`markdown.rs` 内）：每种结构至少一例；包括中文折行宽度、嵌套列表悬挂缩进、
   代码块不折行而截断、`#` 出现在代码块里不被当成标题、行内代码里的 `*` 不被当成强调、
   空输入、只有空白的输入。断言具体的文本与样式（颜色 / modifier），不只是「不 panic」。
