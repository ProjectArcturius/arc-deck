//! 工具调用的**呈现**：按工具类别取摘要、展开输出、把编辑类工具的 `meta.diffs` 画成 diff。
//!
//! 形状从 DSH 源码核对来的，不按 OpenCode 的工具名猜（任务书 w3-tool-views 的要求）：
//! - 文件工具的 `tool/result` 带 `meta = { diffs: [{ path, oldText, newText }], operation }`
//!   （`packages/fs/tool-fs/src/diff.ts` 的 `FsDiffMeta`）；`oldText: null` 是新建文件。
//! - shell 类工具的结果正文末尾可能有 `[exit code: N]`；`fold` 已经把它抽进
//!   [`ToolCall::exit_code`]，这里只负责显示。
//! - 认不出的工具**要有通用回退**，不能因为名字没在表里就什么都不显示。

use arc_deck_core::conversation::{Conversation, Message, ToolCall, ToolStatus};

/// 工具大类。决定摘要长什么样、展开时显示什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `write` / `edit`：带 diff。
    Edit,
    /// `bash` / `pwsh`：命令 + 输出尾。
    Shell,
    /// `read` / `glob` / `grep` 之类：路径 / 模式 + 命中。
    Inspect,
    /// `subagent` / `subagent_fork`：委派。
    Delegated,
    /// 认不出的工具：名字 + 参数首行 + 输出。
    Other,
}

pub(crate) fn kind(name: &str) -> Kind {
    // 名字来自 DSH 的 base bundle（`ls packages/*/tool-*`）：write / edit / read / glob /
    // grep / bash / pwsh / subagent / subagent_fork / todo_write / webfetch / websearch。
    // 一律按**前缀**认：`tool-fs` 的 `edit` 与 `write` 共用一个结果形状。
    match name {
        "write" | "edit" | "apply_patch" => Kind::Edit,
        "bash" | "pwsh" | "shell" | "run" | "run_code" => Kind::Shell,
        "read" | "glob" | "grep" | "list" | "ls" => Kind::Inspect,
        name if name.starts_with("subagent") => Kind::Delegated,
        _ => Kind::Other,
    }
}

/// 一个文件的一次改动，形状与 DSH 的 `FileDiff` 一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileDiff {
    pub path: String,
    /// `None` = 新建文件（`oldText: null`）。
    pub old: Option<String>,
    pub new: Option<String>,
}

/// 从 `tool/result` 的 `meta.diffs` 里取 diff。形状不对就返回空，不猜。
pub(crate) fn diffs_of(call: &ToolCall) -> Vec<FileDiff> {
    let Some(diffs) = call.detail.as_ref().and_then(|meta| meta.get("diffs")).and_then(|d| d.as_array())
    else {
        return Vec::new();
    };
    diffs
        .iter()
        .filter_map(|diff| {
            let path = diff.get("path")?.as_str()?.to_string();
            let text = |key: &str| diff.get(key).and_then(|value| value.as_str()).map(str::to_string);
            Some(FileDiff { path, old: text("oldText"), new: text("newText") })
        })
        .collect()
}

/// 一行摘要里的「关键参数」部分（不含工具名与状态）。渲染层用它把工具名单独上色。
pub(crate) fn subject(call: &ToolCall) -> String {
    match kind(&call.name) {
        Kind::Edit => diffs_of(call)
            .first()
            .map(|diff| diff.path.clone())
            .or_else(|| call.argument("file_path").or_else(|| call.argument("path")))
            .map(|path| format!(" {path}"))
            .unwrap_or_default(),
        Kind::Shell => call
            .argument("command")
            .map(|command| format!(" {}", first_row(&command, 80)))
            .unwrap_or_default(),
        Kind::Inspect => ["path", "file_path", "pattern", "glob"]
            .iter()
            .find_map(|key| call.argument(key))
            .map(|value| format!(" {}", first_row(&value, 60)))
            .unwrap_or_default(),
        Kind::Delegated => call
            .argument("description")
            .map(|value| format!(" {}", first_row(&value, 60)))
            .unwrap_or_default(),
        Kind::Other => ["command", "path", "file_path", "pattern", "url", "query", "description"]
            .iter()
            .find_map(|key| call.argument(key))
            .map(|value| format!(" {}", first_row(&value, 60)))
            .unwrap_or_default(),
    }
}

/// 编辑类工具的 `+N −M`；其他工具为空。
pub(crate) fn counts(call: &ToolCall) -> String {
    if kind(&call.name) != Kind::Edit {
        return String::new();
    }
    let diffs = diffs_of(call);
    if diffs.is_empty() {
        String::new()
    } else {
        let (added, removed) = diff_counts(&diffs);
        format!(" · +{added} −{removed}")
    }
}

/// 成功的工具行前面那个图标，按类别（照 OpenCode：Read `→`、Glob/Grep `✱`、Write/Edit `←`）。
/// 运行中 / 失败的图标由渲染层按状态覆盖。
pub(crate) fn icon(call: &ToolCall) -> &'static str {
    match kind(&call.name) {
        Kind::Edit => "←",
        Kind::Shell => "$",
        Kind::Inspect if call.name == "read" => "→",
        Kind::Inspect => "✱",
        Kind::Delegated => "◉",
        Kind::Other => match call.name.as_str() {
            "webfetch" => "%",
            "websearch" => "◈",
            "todo_write" | "todowrite" => "☐",
            _ => "⚙",
        },
    }
}

/// 工具行前面的符号：运行中 `⋯`、失败（含非零退出）`✗`，其余按类别（[`icon`]）。
pub(crate) fn mark(call: &ToolCall) -> &'static str {
    match call.status {
        ToolStatus::Running => "⋯",
        ToolStatus::Done if call.exit_code.is_some_and(|code| code != 0) => "✗",
        ToolStatus::Failed => "✗",
        _ => icon(call),
    }
}

/// 状态的中文说法。摘要与查看器共用一份，免得两处写法漂移。
pub(crate) fn status_label(call: &ToolCall) -> &'static str {
    match call.status {
        ToolStatus::Running => "进行中",
        ToolStatus::Done if call.exit_code.is_some_and(|code| code != 0) => "非零退出",
        ToolStatus::Done => "完成",
        ToolStatus::Failed => "失败",
        ToolStatus::Abandoned => "未完成",
        ToolStatus::Unknown => "状态未知",
    }
}

/// 增删行数（只看非空行；与 diff 视图的计数口径一致）。
pub(crate) fn diff_counts(diffs: &[FileDiff]) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for diff in diffs {
        let old: Vec<&str> = diff.old.as_deref().unwrap_or("").lines().collect();
        let new: Vec<&str> = diff.new.as_deref().unwrap_or("").lines().collect();
        if diff.old.is_none() {
            added += new.len();
            continue;
        }
        let rows = line_diff(&old, &new, 20_000);
        for row in rows {
            match row.kind {
                DiffKind::Added => added += 1,
                DiffKind::Removed => removed += 1,
                DiffKind::Context => {}
            }
        }
    }
    (added, removed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffRow {
    kind: DiffKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
    text: String,
}

/// 行级 diff（LCS）。`old` / `new` 是**一行一个元素**，不带换行符。
///
/// 为什么自己写而不是引一个 crate：只为了把 DSH 已经切好的 hunk 画出来，LCS 二十行就够；
/// 引一个 diff 库会把「渲染」和「算法」两件事一起带进依赖表。
/// 超过 `limit` 行时退化成「旧的全部删、新的全部加」—— 大文件上 O(n²) 会卡住界面。
fn line_diff(old: &[&str], new: &[&str], limit: usize) -> Vec<DiffRow> {
    if old.len() * new.len() > limit * limit / 4 || old.len().max(new.len()) > limit {
        let mut rows = Vec::new();
        for (index, line) in old.iter().enumerate() {
            rows.push(DiffRow { kind: DiffKind::Removed, old_line: Some(index + 1), new_line: None, text: line.to_string() });
        }
        for (index, line) in new.iter().enumerate() {
            rows.push(DiffRow { kind: DiffKind::Added, old_line: None, new_line: Some(index + 1), text: line.to_string() });
        }
        return rows;
    }
    // 公共前后缀先剥掉：hunk 的绝大部分是上下文，剥完 DP 表通常只剩几行。
    let prefix = old.iter().zip(new.iter()).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];

    let mut rows: Vec<DiffRow> = Vec::new();
    for (index, line) in old.iter().take(prefix).enumerate() {
        rows.push(DiffRow { kind: DiffKind::Context, old_line: Some(index + 1), new_line: Some(index + 1), text: line.to_string() });
    }
    // LCS 表：(n+1) × (m+1)。
    let n = old_mid.len();
    let m = new_mid.len();
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if old_mid[i] == new_mid[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old_mid[i] == new_mid[j] {
            rows.push(DiffRow {
                kind: DiffKind::Context,
                old_line: Some(prefix + i + 1),
                new_line: Some(prefix + j + 1),
                text: old_mid[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            rows.push(DiffRow {
                kind: DiffKind::Removed,
                old_line: Some(prefix + i + 1),
                new_line: None,
                text: old_mid[i].to_string(),
            });
            i += 1;
        } else {
            rows.push(DiffRow {
                kind: DiffKind::Added,
                old_line: None,
                new_line: Some(prefix + j + 1),
                text: new_mid[j].to_string(),
            });
            j += 1;
        }
    }
    while i < n {
        rows.push(DiffRow {
            kind: DiffKind::Removed,
            old_line: Some(prefix + i + 1),
            new_line: None,
            text: old_mid[i].to_string(),
        });
        i += 1;
    }
    while j < m {
        rows.push(DiffRow {
            kind: DiffKind::Added,
            old_line: None,
            new_line: Some(prefix + j + 1),
            text: new_mid[j].to_string(),
        });
        j += 1;
    }
    for (index, line) in old[old.len() - suffix..].iter().enumerate() {
        let line_no = old.len() - suffix + index + 1;
        rows.push(DiffRow {
            kind: DiffKind::Context,
            old_line: Some(line_no),
            new_line: Some(new.len() - suffix + index + 1),
            text: line.to_string(),
        });
    }
    rows
}

/// diff 一行的类别。渲染层按它上色，文本层只看 `DiffLine::text`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffLineKind {
    /// 文件名 / 新建说明。
    Meta,
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffLine {
    pub kind: DiffLineKind,
    /// 已经排好版的一行（含行号与标记）。
    pub text: String,
}

/// 一个文件的 diff（带行号与 `+` / `-` 标记），供渲染层上色。
pub(crate) fn diff_lines(diff: &FileDiff) -> Vec<DiffLine> {
    let mut lines = vec![DiffLine { kind: DiffLineKind::Meta, text: format!("@@ {}", diff.path) }];
    let old: Vec<&str> = diff.old.as_deref().unwrap_or("").lines().collect();
    let new: Vec<&str> = diff.new.as_deref().unwrap_or("").lines().collect();
    if diff.old.is_none() {
        lines.push(DiffLine {
            kind: DiffLineKind::Meta,
            text: format!("（新建，{} 行）", new.len()),
        });
        for (index, line) in new.iter().enumerate() {
            lines.push(DiffLine { kind: DiffLineKind::Added, text: format!("{:>5} + {line}", index + 1) });
        }
        return lines;
    }
    for row in line_diff(&old, &new, 20_000) {
        let marker = match row.kind {
            DiffKind::Context => ' ',
            DiffKind::Added => '+',
            DiffKind::Removed => '-',
        };
        let kind = match row.kind {
            DiffKind::Context => DiffLineKind::Context,
            DiffKind::Added => DiffLineKind::Added,
            DiffKind::Removed => DiffLineKind::Removed,
        };
        let old_no = row.old_line.map(|n| n.to_string()).unwrap_or_default();
        let new_no = row.new_line.map(|n| n.to_string()).unwrap_or_default();
        lines.push(DiffLine { kind, text: format!("{old_no:>5} {new_no:>5} {marker} {}", row.text) });
    }
    lines
}

/// 纯文本版本（查看器用）。
pub(crate) fn diff_text(diff: &FileDiff) -> Vec<String> {
    diff_lines(diff).into_iter().map(|line| line.text).collect()
}

/// 本会话里所有编辑类工具的 diff（按发生顺序，同一文件合并成一节）。
pub(crate) fn session_diff(conversation: &Conversation) -> Vec<String> {
    let mut lines = Vec::new();
    for call in tool_calls(conversation) {
        if kind(&call.name) != Kind::Edit {
            continue;
        }
        for diff in diffs_of(call) {
            lines.extend(diff_text(&diff));
        }
    }
    lines
}

/// 本会话改动过的文件（去重、按首次出现排序）。
pub(crate) fn changed_files(conversation: &Conversation) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for call in tool_calls(conversation) {
        for diff in diffs_of(call) {
            if !files.contains(&diff.path) {
                files.push(diff.path);
            }
        }
    }
    files
}

/// 全部工具调用，按出现顺序。
pub(crate) fn tool_calls(conversation: &Conversation) -> Vec<&ToolCall> {
    conversation
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Assistant(turn) => Some(&turn.blocks),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            arc_deck_core::conversation::Block::Tool(call) => Some(call),
            _ => None,
        })
        .collect()
}

/// 完整输出查看器里的文本：标题信息 + 参数 + 结果正文。
pub(crate) fn output_text(call: &ToolCall) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("工具：{}（{}）", call.name, status_label(call)));
    if let Some(code) = call.exit_code {
        lines.push(format!("退出码：{code}"));
    }
    if !call.arguments.trim().is_empty() {
        lines.push(String::new());
        lines.push("参数：".into());
        match serde_json::from_str::<serde_json::Value>(&call.arguments) {
            Ok(value) => {
                // 格式化过的 JSON 好读；解析不了就原样放，别假装它坏掉了。
                for line in serde_json::to_string_pretty(&value).unwrap_or_else(|_| call.arguments.clone()).lines() {
                    lines.push(line.to_string());
                }
            }
            Err(_) => lines.push(call.arguments.clone()),
        }
    }
    if let Some(error) = &call.error {
        lines.push(String::new());
        lines.push(format!("错误：{error}"));
    }
    if let Some(output) = &call.output {
        lines.push(String::new());
        lines.push("结果：".into());
        for line in output.lines() {
            lines.push(line.to_string());
        }
    }
    if lines.len() == 1 {
        lines.push("（这个工具没有输出）".into());
    }
    lines
}

/// 一行里最多多少字符（按**字符**算，不是字节：中文一字节切开会变成替换字符）。
pub(crate) fn first_row(text: &str, max: usize) -> String {
    let row = text.lines().next().unwrap_or("");
    if row.chars().count() > max {
        format!("{}…", row.chars().take(max).collect::<String>())
    } else {
        row.to_string()
    }
}

/// 折叠时显示的输出尾部行数：失败多给几行（拒因 / 栈顶通常在前面）。
pub(crate) fn folded_lines(call: &ToolCall) -> usize {
    let failed = matches!(call.status, ToolStatus::Failed) || call.exit_code.is_some_and(|code| code != 0);
    if failed {
        4
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_deck_core::conversation::{AssistantTurn, Block, Message};

    fn call(name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            call_id: "c".into(),
            name: name.into(),
            arguments: arguments.into(),
            status: ToolStatus::Done,
            ..ToolCall::default()
        }
    }

    #[test]
    fn tool_kinds_follow_the_dsh_tool_names() {
        assert_eq!(kind("write"), Kind::Edit);
        assert_eq!(kind("edit"), Kind::Edit);
        assert_eq!(kind("bash"), Kind::Shell);
        assert_eq!(kind("pwsh"), Kind::Shell);
        assert_eq!(kind("grep"), Kind::Inspect);
        assert_eq!(kind("subagent_fork"), Kind::Delegated);
        assert_eq!(kind("todo_write"), Kind::Other, "认不出要有通用回退");
    }

    #[test]
    fn a_new_file_diff_is_all_additions_and_sums_up() {
        let mut tool = call("write", r#"{"path":"a.txt"}"#);
        tool.detail = Some(serde_json::json!({
            "diffs": [{ "path": "a.txt", "oldText": null, "newText": "一\n二" }]
        }));
        let diffs = diffs_of(&tool);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].old, None);
        assert_eq!(diff_counts(&diffs), (2, 0));
        let text = diff_text(&diffs[0]).join("\n");
        assert!(text.contains("+ 一") && text.contains("新建"), "{text}");
    }

    #[test]
    fn an_edit_diff_shows_both_sides_with_line_numbers() {
        let diff = FileDiff {
            path: "x.rs".into(),
            old: Some("a\nb\nc".into()),
            new: Some("a\nB\nc".into()),
        };
        let text = diff_text(&diff).join("\n");
        assert!(text.contains("@@ x.rs"), "{text}");
        assert!(text.contains("- b") || text.contains("-b"), "{text}");
        assert!(text.contains("+ B") || text.contains("+B"), "{text}");
        assert!(text.contains("  a"), "上下文行要留着：{text}");
        assert_eq!(diff_counts(&[diff]), (1, 1));
    }

    /// 大文件不能走 O(n²)：退化路径仍要给出可读的「全删 + 全加」。
    #[test]
    fn a_huge_hunk_degrades_instead_of_quadratic() {
        let old: Vec<&str> = (0..3000).map(|_| "same").collect();
        let new: Vec<&str> = (0..3000).map(|_| "other").collect();
        let rows = line_diff(&old, &new, 4000);
        assert_eq!(rows.len(), 6000);
    }

    #[test]
    fn the_summary_is_made_of_separate_parts() {
        let mut shell = call("bash", r#"{"command":"cargo test\n第二行"}"#);
        assert!(subject(&shell).contains("cargo test"));
        assert_eq!(status_label(&shell), "完成");
        assert_eq!(mark(&shell), "$");
        shell.status = ToolStatus::Failed;
        shell.exit_code = Some(101);
        assert_eq!(status_label(&shell), "失败");
        assert_eq!(mark(&shell), "✗");
    }

    /// 子 agent 与认不出的工具也要有摘要与输出；不能因为没在表里就消失。
    #[test]
    fn unknown_and_delegated_tools_still_render() {
        let mut delegated = call("subagent", r#"{"description":"查一下"}"#);
        delegated.output = Some("done".into());
        assert!(subject(&delegated).contains("查一下"));
        assert_eq!(mark(&delegated), "◉");
        assert!(output_text(&delegated).join("\n").contains("done"));

        let unknown = call("mystery", r#"{"query":"x"}"#);
        assert_eq!(mark(&unknown), "⚙");
        assert!(!output_text(&unknown).is_empty());
    }

    #[test]
    fn changed_files_are_deduped_in_order() {
        let mut first = call("edit", r#"{"path":"b"}"#);
        first.detail = Some(serde_json::json!({"diffs":[{"path":"b","oldText":"x","newText":"y"}]}));
        let mut second = call("edit", r#"{"path":"a"}"#);
        second.detail = Some(serde_json::json!({"diffs":[{"path":"a","oldText":null,"newText":"z"}]}));
        let mut again = call("edit", r#"{"path":"b"}"#);
        again.detail = Some(serde_json::json!({"diffs":[{"path":"b","oldText":"y","newText":"z"}]}));
        let conversation = Conversation {
            messages: vec![Message::Assistant(AssistantTurn {
                turn: 1,
                blocks: vec![
                    Block::Tool(first),
                    Block::Tool(second),
                    Block::Tool(again),
                ],
                ..AssistantTurn::default()
            })],
            ..Conversation::default()
        };
        assert_eq!(changed_files(&conversation), vec!["b".to_string(), "a".to_string()]);
        assert!(!session_diff(&conversation).is_empty());
    }

    /// 参数里带换行时摘要只取第一行（否则一行摘要会变成一块）。
    #[test]
    fn summaries_take_one_row() {
        let shell = call("bash", "{\"command\":\"第一行\\n第二行\"}");
        let line = subject(&shell);
        assert!(!line.contains("第二行"), "{line}");
        assert!(line.contains("第一行"));
    }
}
