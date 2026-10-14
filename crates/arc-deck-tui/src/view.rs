//! Home/composer/sidebar proportions follow OpenCode v1.19.0, not its session tabs.
//! Copyright (c) 2025 opencode. MIT notice: ../LICENSE-OpenCode.
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, Dialog, token},
    theme::Theme,
};

pub(crate) fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let theme = Theme::new(app.preferences.light);
    frame.render_widget(
        Block::new().style(Style::default().fg(theme.text).bg(theme.background)),
        area,
    );
    if area.width < 20 || area.height < 8 {
        frame.render_widget(Paragraph::new("终端过小\n至少 20×8\nCtrl+C 退出"), area);
        return;
    }
    let footer = Rect::new(area.x + 1, area.bottom() - 2, area.width - 2, 2);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                clean(&app.notice),
                Style::default().fg(theme.feedback_warning),
            ),
            Line::styled(
                if app.leader {
                    "leader: n 新请求 · l 历史 · a Agent · m 模型 · b 信息栏"
                } else if cfg!(windows) && area.width < 65 {
                    "F6发送 Enter换行 ^P命令 /exit退出"
                } else if cfg!(windows) {
                    "F6 发送 · Enter 换行 · ^P 命令 · ^X leader · PgUp 上滚 · Esc 取消 · /exit 退出"
                } else if area.width < 65 {
                    "Enter发送 ^J换行 ^P命令 ^C退出"
                } else {
                    "Enter 发送 · ^J 换行 · ^P 命令 · ^X leader · PgUp 上滚 · Esc 取消 · ^C 退出"
                },
                Style::default().fg(theme.text_muted),
            ),
        ]),
        footer,
    );
    let content = Rect::new(area.x, area.y, area.width, area.height - 2);
    if app.selected.is_none() {
        home(frame, app, content, &theme);
    } else {
        session(frame, app, content, &theme);
    }
    if app.sidebar_overlay
        && app.preferences.sidebar
        && content.width <= 120
        && app.selected.is_some()
    {
        let w = 42.min(content.width - 2);
        sidebar(
            frame,
            app,
            Rect::new(content.right() - w, content.y, w, content.height),
            &theme,
        );
    }
    if let Some(dialog) = app.dialog {
        popup(frame, app, dialog, content, &theme);
    }
}

fn home(frame: &mut Frame, app: &App, area: Rect, t: &Theme) {
    let width = 75.min(area.width.saturating_sub(4));
    let x = area.x + (area.width - width) / 2;
    let height = 9.min(area.height);
    let y = area.y + area.height.saturating_sub(height) / 2;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Arc ", Style::default().fg(t.text_muted)),
            Span::styled(
                "Deck",
                Style::default().fg(t.action).add_modifier(Modifier::BOLD),
            ),
        ])),
        Rect::new(x, y, width, 1),
    );
    frame.render_widget(
        Paragraph::new("从当前项目开始一次独立请求").style(Style::default().fg(t.text_muted)),
        Rect::new(x, y + 2, width, 1),
    );
    composer(
        frame,
        app,
        Rect::new(x, y + 3, width, height.saturating_sub(3)),
        t,
    );
    if area.height > 16 {
        frame.render_widget(
            Paragraph::new(clean(&app.boot.workspace.display().to_string()))
                .style(Style::default().fg(t.text_muted)),
            Rect::new(x, y + height + 1, width, 1),
        );
    }
}

fn session(frame: &mut Frame, app: &mut App, area: Rect, t: &Theme) {
    let side = if app.preferences.sidebar && area.width > 120 {
        42
    } else {
        0
    };
    let main = Rect::new(area.x, area.y, area.width - side, area.height);
    if side > 0 {
        sidebar(
            frame,
            app,
            Rect::new(main.right(), area.y, side, area.height),
            t,
        );
    }
    let label = app
        .detail
        .as_ref()
        .map(|d| d.record.title.as_str())
        .unwrap_or("正在接入运行记录…");
    frame.render_widget(
        Paragraph::new(clean(&format!(" Arc Deck  /  {label}")))
            .style(Style::default().fg(t.text).bg(t.background_panel)),
        Rect::new(main.x, main.y, main.width, 1),
    );
    let width = main.width.saturating_sub(4).max(2);
    let rows = app.editor.wrapped(width as usize).0.len() as u16;
    let composer_height = (rows + 3).clamp(5, 9).min(main.height.saturating_sub(3));
    let body = Rect::new(
        main.x + 2,
        main.y + 2,
        width,
        main.height.saturating_sub(composer_height + 3),
    );
    let lines = body_lines(app, t);
    let lines = wrap(lines, body.width as usize);
    app.scroll_max = lines.len().saturating_sub(body.height as usize);
    let top = app.scroll.unwrap_or(app.scroll_max).min(app.scroll_max);
    // Explicit wrapped rows avoid estimating Ratatui's line count: an estimate caused
    // CJK rows to move the user's scroll anchor whenever progress was appended.
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(top)
                .take(body.height as usize)
                .collect::<Vec<_>>(),
        ),
        body,
    );
    composer(
        frame,
        app,
        Rect::new(
            main.x + 1,
            main.bottom() - composer_height,
            main.width - 2,
            composer_height,
        ),
        t,
    );
}

fn composer(frame: &mut Frame, app: &App, area: Rect, t: &Theme) {
    if area.height < 3 || area.width < 4 {
        return;
    }
    frame.render_widget(
        Block::new()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(t.action))
            .style(Style::default().bg(t.formfield)),
        area,
    );
    let inner = Rect::new(area.x + 2, area.y + 1, area.width - 3, area.height - 3);
    let (rows, (cx, cy)) = app.editor.wrapped(inner.width as usize);
    let start = cy.saturating_sub(inner.height.saturating_sub(1) as usize);
    let visible = if app.editor.text.is_empty() {
        vec!["输入指令…每次提交创建独立运行".into()]
    } else {
        rows.into_iter()
            .skip(start)
            .take(inner.height as usize)
            .collect()
    };
    frame.render_widget(
        Paragraph::new(visible.join("\n")).style(Style::default().fg(
            if app.editor.text.is_empty() {
                t.text_muted
            } else {
                t.text
            },
        )),
        inner,
    );
    let mut metadata = format!("{} · {}", app.agent, app.permission);
    if inner.width > 42
        && let Some(model) = &app.boot.model
    {
        metadata.push_str(&format!(" · {model}"));
    }
    if inner.width > 63
        && let Some(effort) = &app.effort
    {
        metadata.push_str(&format!(" · effort {effort}"));
    }
    frame.render_widget(
        Paragraph::new(metadata).style(Style::default().fg(t.action)),
        Rect::new(inner.x, area.bottom() - 2, inner.width, 1),
    );
    let overlay = app.sidebar_overlay
        && app.preferences.sidebar
        && frame.area().width <= 120
        && app.selected.is_some();
    if app.dialog.is_none() && !overlay && inner.height > 0 {
        frame.set_cursor_position((
            inner.x + (cx as u16).min(inner.width - 1),
            inner.y + (cy - start) as u16,
        ));
    }
}

fn sidebar(frame: &mut Frame, app: &App, area: Rect, t: &Theme) {
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::new()
            .style(Style::default().bg(t.background_panel))
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(t.border)),
        area,
    );
    let inner = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(3),
        area.height.saturating_sub(2),
    );
    let mut lines = vec![
        Line::styled("工作区", Style::default().fg(t.action)),
        Line::raw(clean(&app.boot.workspace.display().to_string())),
        Line::raw(""),
    ];
    if let Some(d) = &app.detail {
        for (name, value) in [
            ("运行", d.record.id.as_str()),
            ("状态", d.record.state.as_str()),
            ("健康", d.record.health.as_str()),
            ("活动", d.record.activity.as_str()),
            ("模型", d.record.model.as_deref().unwrap_or("未记录")),
        ] {
            lines.push(Line::styled(name, Style::default().fg(t.text_muted)));
            lines.push(Line::raw(clean(value)));
        }
        if let Some(u) = &d.record.usage {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "用量（provider 实报）",
                Style::default().fg(t.action),
            ));
            lines.push(Line::raw(format!(
                "输入 {} · 输出 {}",
                token(u.prompt_tokens),
                token(u.completion_tokens)
            )));
            lines.push(Line::raw(format!("缓存 {}", token(u.cache_read_tokens))));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "证据 /evidence",
            Style::default().fg(t.action),
        ));
        lines.push(Line::raw(clean(&d.evidence_dir.display().to_string())));
    }
    frame.render_widget(
        Paragraph::new(
            wrap(lines, inner.width as usize)
                .into_iter()
                .take(inner.height as usize)
                .collect::<Vec<_>>(),
        ),
        inner,
    );
}

fn body_lines(app: &App, t: &Theme) -> Vec<Line<'static>> {
    let Some(d) = &app.detail else {
        return vec![Line::styled(
            "等待监工写入状态…",
            Style::default().fg(t.text_muted),
        )];
    };
    let mut lines = vec![Line::styled(
        "你",
        Style::default().fg(t.action).add_modifier(Modifier::BOLD),
    )];
    lines.extend(
        clean(&d.instruction)
            .lines()
            .map(|s| Line::raw(s.to_string())),
    );
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        format!(
            "{} · {} · {}",
            d.record.state,
            d.record.health,
            clean(&d.record.activity)
        ),
        Style::default().fg(if d.record.health == "dead" || d.record.state == "failed" {
            t.feedback_error
        } else {
            t.text_muted
        }),
    ));
    if let Some(notice) = &d.record.notice {
        lines.push(Line::styled(
            clean(notice),
            Style::default().fg(t.feedback_warning),
        ));
    }
    let expanded = app.expanded.get(&d.record.id).copied().unwrap_or(false);
    let mut tools: Vec<(String, String, String, String)> = Vec::new();
    let mut indexes = HashMap::new();
    for event in &d.events {
        let str_at = |name| {
            event
                .data
                .get(name)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        if event.kind == "tool_call" {
            let id = str_at("call_id");
            indexes.insert(id.clone(), tools.len());
            tools.push((id, str_at("tool"), "running".into(), str_at("summary")));
        } else if event.kind == "tool_result" {
            if let Some(i) = indexes.get(&str_at("call_id")) {
                tools[*i].2 = str_at("status");
                tools[*i].3.push_str(&format!("\n{}", str_at("summary")));
            } else {
                tools.push((
                    str_at("call_id"),
                    str_at("tool"),
                    str_at("status"),
                    str_at("summary"),
                ));
            }
        } else if event.kind == "diagnostic" && matches!(str_at("level").as_str(), "error" | "warn")
        {
            lines.push(Line::styled(
                clean(&str_at("message")),
                Style::default().fg(t.feedback_warning),
            ));
        }
    }
    if !tools.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!(
                "工具 · {} 条 · /tools {}",
                tools.len(),
                if expanded { "折叠" } else { "展开" }
            ),
            Style::default().fg(t.text_muted),
        ));
        for (_, name, state, summary) in tools {
            let color = match state.as_str() {
                "success" | "ok" => t.feedback_success,
                "error" | "failed" | "denied" => t.feedback_error,
                _ => t.text_muted,
            };
            lines.push(Line::styled(
                format!("  {name} · {state}"),
                Style::default().fg(color),
            ));
            // Failure detail stays visible even when normal output is folded.
            if expanded || matches!(state.as_str(), "error" | "failed" | "denied") {
                lines.extend(
                    clean(&summary)
                        .lines()
                        .map(|s| Line::raw(format!("    {s}"))),
                );
            }
        }
    }
    if let Some(r) = &d.result {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Arc Deck 采集的事实",
            Style::default().fg(t.action),
        ));
        lines.push(Line::raw(format!("终态：{}", r.status.as_str())));
        for b in &r.blockers {
            lines.push(Line::styled(
                clean(&format!("{}: {}", b.code, b.message)),
                Style::default().fg(t.feedback_error),
            ));
        }
        for v in &r.verification {
            lines.push(Line::raw(clean(&format!(
                "验收 {} · {:?} · exit {:?}",
                v.id, v.outcome, v.exit_code
            ))));
        }
        for text in &r.unverified {
            lines.push(Line::styled(
                clean(text),
                Style::default().fg(t.feedback_warning),
            ));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled("模型的陈述", Style::default().fg(t.action)));
        lines.extend(markdown(d.final_text.as_deref().unwrap_or(&r.summary), t));
    } else if d.record.terminal {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "状态已终止，但结果文件不可读；请检查 /evidence",
            Style::default().fg(t.feedback_warning),
        ));
    } else {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "等待最终正文；此通道没有实时 token 输出",
            Style::default().fg(t.text_muted),
        ));
    }
    if d.truncated {
        lines.push(Line::styled(
            "显示内容已截断；完整内容见 /evidence",
            Style::default().fg(t.feedback_warning),
        ));
    }
    lines
}

fn markdown(text: &str, t: &Theme) -> Vec<Line<'static>> {
    let mut code = false;
    clean(text)
        .lines()
        .map(|s| {
            if s.starts_with("```") {
                code = !code;
                Line::styled(s.to_string(), Style::default().fg(t.text_muted))
            } else if code {
                Line::styled(
                    s.to_string(),
                    Style::default().bg(t.background_element).fg(t.text),
                )
            } else if s.starts_with('#') {
                Line::styled(
                    s.to_string(),
                    Style::default().fg(t.action).add_modifier(Modifier::BOLD),
                )
            } else {
                Line::raw(s.to_string())
            }
        })
        .collect()
}

fn popup(frame: &mut Frame, app: &App, dialog: Dialog, area: Rect, t: &Theme) {
    let w = 72.min(area.width - 2);
    let h = 18.min(area.height.saturating_sub(2)).max(4);
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    let title = match dialog {
        Dialog::Commands => "命令",
        Dialog::Runs => "运行历史",
        Dialog::Agents => "Agent",
        Dialog::Models => "模型路由",
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::bordered()
            .title(title)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(t.background_panel)),
        rect,
    );
    let inner = Rect::new(rect.x + 2, rect.y + 1, rect.width - 4, rect.height - 2);
    let mut rows: Vec<(String, bool)> = match dialog {
        Dialog::Commands => app
            .filtered_commands()
            .iter()
            .map(|c| (format!("{}  {}", c.slash, c.label), true))
            .collect(),
        Dialog::Runs => app
            .filtered_runs()
            .iter()
            .map(|r| (format!("{} · {} · {}", r.title, r.state, r.health), true))
            .collect(),
        Dialog::Agents => vec![
            ("Echo · 默认工程角色".into(), true),
            ("Build · 实施".into(), true),
            ("Plan · 不可用：执行 guard 尚未接通".into(), false),
        ],
        Dialog::Models => vec![(
            format!(
                "{} · 配置路由（运行时校验）",
                app.boot.model.as_deref().unwrap_or("未配置")
            ),
            false,
        )],
    };
    if rows.is_empty() {
        rows.push(("没有匹配项".into(), false));
    }
    let search = if matches!(dialog, Dialog::Commands | Dialog::Runs) {
        format!("搜索：{}", app.filter)
    } else {
        "↑↓ 选择 · Enter 确定 · Esc 返回".into()
    };
    frame.render_widget(
        Paragraph::new(search).style(Style::default().fg(t.text_muted)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let start = app
        .choice
        .saturating_sub(inner.height.saturating_sub(3) as usize);
    let lines = rows
        .into_iter()
        .enumerate()
        .skip(start)
        .take(inner.height.saturating_sub(2) as usize)
        .map(|(i, (row, enabled))| {
            let style = if i == app.choice {
                Style::default()
                    .fg(if enabled {
                        t.action_selected
                    } else {
                        t.action_disabled
                    })
                    .bg(t.background_element)
            } else {
                Style::default().fg(if enabled { t.text } else { t.action_disabled })
            };
            Line::styled(
                clean(&format!(
                    "{} {row}",
                    if i == app.choice { "›" } else { " " }
                )),
                style,
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(
            inner.x,
            inner.y + 2,
            inner.width,
            inner.height.saturating_sub(2),
        ),
    );
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}

fn wrap(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(2);
    let mut result = Vec::new();
    for line in lines {
        let mut row = Line::default();
        let mut x = 0;
        for span in line.spans {
            for g in span.content.graphemes(true) {
                let n = g.width();
                if x + n > width {
                    result.push(row);
                    row = Line::default();
                    x = 0;
                }
                row.spans
                    .push(Span::styled(g.to_string(), span.style.patch(line.style)));
                x += n;
            }
        }
        result.push(row);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Boot, Preferences, RunDetail, RunRecord};
    use arc_deck_core::{
        event::EventEnvelope,
        result::{TaskResult, TaskStatus},
    };
    use ratatui::{Terminal, backend::TestBackend};
    fn app() -> App {
        App::new(
            Boot {
                workspace: "C:/项目/arc-deck".into(),
                model: Some("opencode-go/deepseek-v4.1-flash".into()),
                effort: None,
                config_error: None,
                preferences: "unused".into(),
            },
            Preferences {
                sidebar: true,
                ..Preferences::default()
            },
        )
    }
    fn long_run(app: &mut App) {
        let result = TaskResult::new(
            "r",
            TaskStatus::Failed,
            (0..100)
                .map(|i| format!("第{i}行中文结果\n"))
                .collect::<String>(),
        );
        let record = RunRecord {
            id: "r".into(),
            title: "验证工具失败".into(),
            state: "failed".into(),
            health: "done".into(),
            activity: "idle".into(),
            model: app.boot.model.clone(),
            started_at: 1,
            terminal: true,
            usage: None,
            notice: None,
        };
        app.selected = Some("r".into());
        app.detail = Some(Box::new(RunDetail {
            record,
            instruction: "运行一条验证命令".into(),
            events: vec![
                EventEnvelope::tool_call("r", 1, "id", "bash", "cargo test"),
                EventEnvelope::tool_result("r", 2, "id", "bash", "error", "exit 101"),
            ],
            result: Some(result),
            final_text: None,
            evidence_dir: "C:/state/runs/r".into(),
            truncated: false,
        }));
    }
    #[test]
    fn all_target_sizes_render_home_run_dialogs_and_both_themes() {
        for (width, height) in [(40, 12), (80, 24), (120, 30), (160, 40)] {
            for light in [false, true] {
                let mut a = app();
                a.preferences.light = light;
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|f| draw(f, &mut a)).unwrap();
                let buffer = terminal.backend().buffer();
                assert!(buffer.content.iter().any(|c| c.symbol() == "A"));
                long_run(&mut a);
                for dialog in [
                    None,
                    Some(Dialog::Commands),
                    Some(Dialog::Runs),
                    Some(Dialog::Agents),
                    Some(Dialog::Models),
                ] {
                    a.dialog = dialog;
                    terminal.draw(|f| draw(f, &mut a)).unwrap();
                }
                assert_eq!(
                    terminal.backend().buffer().area,
                    Rect::new(0, 0, width, height)
                );
            }
        }
    }
    #[test]
    fn failure_details_are_visible_even_when_tools_are_folded() {
        let mut a = app();
        long_run(&mut a);
        let text = body_lines(&a, &Theme::new(false))
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("exit 101"));
        assert_eq!(text.matches("第0行中文结果").count(), 1);
        assert!(text.contains("Arc Deck 采集的事实"));
        assert!(text.contains("模型的陈述"));
    }
    #[test]
    fn cjk_cursor_uses_terminal_cells_after_wrap() {
        let mut a = app();
        a.editor.insert("中文e\u{301}");
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let cursor = terminal.get_cursor_position().unwrap();
        assert_eq!(cursor.x, 9);
        assert!(cursor.y < 12);
    }
    #[test]
    fn semantic_themes_do_not_use_failure_color_for_normal_text() {
        for light in [false, true] {
            let t = Theme::new(light);
            assert_ne!(t.text, t.feedback_error);
            assert_ne!(t.background, t.formfield);
        }
    }
    #[test]
    fn generate_reviewable_cell_snapshots() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tui-preview");
        std::fs::create_dir_all(&dir).unwrap();
        for (w, h) in [(40, 12), (80, 24), (120, 30), (160, 40)] {
            for light in [false, true] {
                for screen in ["home", "run", "commands"] {
                    let mut a = app();
                    a.preferences.light = light;
                    if screen != "home" {
                        long_run(&mut a);
                    }
                    if screen == "commands" {
                        a.open_dialog(Dialog::Commands);
                    }
                    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
                    terminal.draw(|f| draw(f, &mut a)).unwrap();
                    let b = terminal.backend().buffer();
                    let mut text = String::new();
                    for y in 0..h {
                        let mut x = 0;
                        while x < w {
                            let glyph = b[(x, y)].symbol();
                            text.push_str(glyph);
                            x += (glyph.width() as u16).max(1);
                        }
                        text.push('\n');
                    }
                    std::fs::write(
                        dir.join(format!(
                            "{screen}-{w}x{h}-{}.txt",
                            if light { "light" } else { "dark" }
                        )),
                        text,
                    )
                    .unwrap();
                }
            }
        }
    }
}
