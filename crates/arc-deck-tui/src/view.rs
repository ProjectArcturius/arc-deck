//! Home/composer/sidebar proportions follow OpenCode v1.19.0, not its session tabs.
//! Copyright (c) 2025 opencode. MIT notice: ../LICENSE-OpenCode.
//!
//! Colours come from the v2.0.27 semantic tokens (`theme.rs`). Most call sites still want the
//! flat role names the old hand-written `Theme` had, so the small helpers at the bottom of this
//! file are the single place where a role is spelled as a token plus a state — the alternative
//! was repeating `text.action(primary, &[])` at forty call sites and drifting.
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, Dialog, token},
    markdown::{self, MarkdownPalette},
    model_dialog,
    theme::{Feedback, State, Theme, Variant},
    theme_dialog,
};

/// `text.action.secondary.base` — the flat "normal text" role, distinct from `text.base`.
pub(crate) fn muted(t: &Theme) -> Color {
    t.text_action(Variant::Secondary, &[]).over(t.background)
}

/// `text.action.primary.base`: the theme's accent. Upstream's v1 migration points primary at
/// `$text.base`, which would make it invisible, so the *selected* state (the interactive hue)
/// is what reads as an accent for body chrome such as brand text and section labels.
pub(crate) fn accent(t: &Theme) -> Color {
    t.text_action(Variant::Primary, &[State::Selected]).over(t.background)
}

/// `background.raised.base` — the secondary surface used by the sidebar and dialogs. This is
/// what the old hand-written palette's `background_panel` maps onto.
pub(crate) fn panel_deep(t: &Theme) -> Color {
    t.background_raised.over(t.background)
}

/// `background.raised.high` — one step up from `panel_deep`. This is the token the old
/// hand-written palette's `background_element` maps onto: `background.action.primary.$hovered`
/// is the fill for a *hovered primary action*, and painting a code block with it made the block
/// the same shade as the panel it sits on.
pub(crate) fn panel_highlight(t: &Theme) -> Color {
    t.background_raised_high.over(t.background_raised)
}

/// `text.base` 铺在页面底色上 —— 旧调色板的 `text`。
pub(crate) fn text(t: &Theme) -> Color {
    t.text.over(t.background)
}

/// `text.action.primary.$disabled` —— 旧调色板的 `action_disabled`（映射与测试见下方
/// `old_fields_map_to_pinned_tokens` 一类的断言）。
pub(crate) fn disabled(t: &Theme) -> Color {
    t.text_action(Variant::Primary, &[State::Disabled]).over(t.background)
}

/// 对话框底色：与 `popup` 画的那一块同一个 token，模型对话框不另取一份。
pub(crate) fn dialog_bg(t: &Theme) -> Color {
    t.dialog_background(t.background_raised).over(t.background)
}

pub(crate) fn warning(t: &Theme) -> Color {
    t.text_feedback(Feedback::Warning, false).over(t.background)
}

fn danger(t: &Theme) -> Color {
    t.text_feedback(Feedback::Error, false).over(t.background)
}

fn ok(t: &Theme) -> Color {
    t.text_feedback(Feedback::Success, false).over(t.background)
}

/// The foreground/background pair for the highlighted row of a dialog, painted the way upstream's
/// `DialogSelect` paints it: the same `$focused` state on both halves of the `action.primary`
/// pair. (An earlier version used `$selected` on both halves, and in every built-in theme those
/// resolve to the same `$hue.interactive.200` — the row was painted in its own background and
/// read as blank. `every_built_in_theme_draws_the_picker_in_both_modes` and the sibling test in
/// `theme_dialog` are the guards.)
///
/// Shared with `theme_dialog` so the app has one "the cursor is here" treatment rather than one
/// per dialog.
pub(crate) fn row_focus(t: &Theme) -> (Color, Color) {
    let surface = t.dialog_background(t.background_raised);
    (
        t.text_action(Variant::Primary, &[State::Focused]).over(surface),
        t.background_action(Variant::Primary, &[State::Focused]).over(surface),
    )
}

pub(crate) fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let theme = app.theme.clone();
    let t = &theme;
    frame.render_widget(
        Block::new().style(Style::default().fg(t.text.to_ratatui()).bg(t.background.to_ratatui())),
        area,
    );
    if area.width < 20 || area.height < 8 {
        frame.render_widget(Paragraph::new("终端过小\n至少 20×8\nCtrl+C 退出"), area);
        return;
    }
    let footer = Rect::new(area.x + 1, area.bottom() - 2, area.width - 2, 2);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(clean(&app.notice), Style::default().fg(warning(t))),
            Line::styled(
                if app.leader {
                    "leader: n 新请求 · l 历史 · a Agent · m 模型 · b 信息栏 · t 主题"
                } else if cfg!(windows) && area.width < 65 {
                    "F6发送 Enter换行 ^P命令 /exit退出"
                } else if cfg!(windows) {
                    "F6 发送 · Enter 换行 · ^P 命令 · ^X leader · PgUp 上滚 · Esc 取消 · /theme 主题 · /exit 退出"
                } else if area.width < 65 {
                    "Enter发送 ^J换行 ^P命令 ^C退出"
                } else {
                    "Enter 发送 · ^J 换行 · ^P 命令 · ^X leader · PgUp 上滚 · Esc 取消 · /theme 主题 · ^C 退出"
                },
                Style::default().fg(muted(t)),
            ),
        ]),
        footer,
    );
    let content = Rect::new(area.x, area.y, area.width, area.height - 2);
    if app.selected.is_none() {
        home(frame, app, content, t);
    } else {
        session(frame, app, content, t);
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
            t,
        );
    }
    if let Some(dialog) = &app.dialog {
        popup(frame, app, dialog, content, t);
    }
}

fn home(frame: &mut Frame, app: &App, area: Rect, t: &Theme) {
    let width = 75.min(area.width.saturating_sub(4));
    let x = area.x + (area.width - width) / 2;
    let height = 9.min(area.height);
    let y = area.y + area.height.saturating_sub(height) / 2;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Arc ", Style::default().fg(muted(t))),
            Span::styled(
                "Deck",
                Style::default().fg(accent(t)).add_modifier(Modifier::BOLD),
            ),
        ])),
        Rect::new(x, y, width, 1),
    );
    frame.render_widget(
        Paragraph::new("从当前项目开始一次独立请求").style(Style::default().fg(muted(t))),
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
                .style(Style::default().fg(muted(t))),
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
            .style(Style::default().fg(t.text.to_ratatui()).bg(panel_deep(t))),
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
    let lines = body_lines(app, t, body.width);
    let lines = fit_width(lines, body.width as usize);
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
    // `background.formfield.base` is `transparent` in OpenCode's own themes, which is a real
    // change from the old hand-written palette (a `#1e1e1e` input tint). Keeping the token as
    // the theme defines it is the point of the migration; the left rule plus the muted
    // placeholder still mark the input, and `over` resolves transparency against the base.
    frame.render_widget(
        Block::new()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(accent(t)))
            .style(Style::default().bg(t.background_formfield(&[]).over(t.background))),
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
    // The composer *is* the form field, so its text colour is `text.formfield`, not `text` —
    // the token group exists for exactly this element. Both resolve to `$hue.neutral.200` in
    // OpenCode's own theme, so this is a naming fix rather than a visual change.
    frame.render_widget(
        Paragraph::new(visible.join("\n")).style(Style::default().fg(
            if app.editor.text.is_empty() {
                muted(t)
            } else {
                t.text_formfield(&[]).over(t.background)
            },
        )),
        inner,
    );
    let mut metadata = format!("{} · {}", app.agent, app.permission);
    // 模型与 effort 都显示**当前值**（由模型对话框改动），不是配置里的那份 ——
    // 这一行是用户确认「下一次请求到底用什么」的地方。
    if inner.width > 42 {
        metadata.push_str(&format!(" · {}", app.model_name()));
    }
    if inner.width > 63
        && let Some(effort) = &app.effort
    {
        metadata.push_str(&format!(" · effort {effort}"));
    }
    frame.render_widget(
        Paragraph::new(metadata).style(Style::default().fg(accent(t))),
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
            .style(Style::default().bg(panel_deep(t)))
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(t.border.to_ratatui())),
        area,
    );
    let inner = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(3),
        area.height.saturating_sub(2),
    );
    let mut lines = vec![
        Line::styled("工作区", Style::default().fg(accent(t))),
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
            lines.push(Line::styled(name, Style::default().fg(muted(t))));
            lines.push(Line::raw(clean(value)));
        }
        if let Some(u) = &d.record.usage {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "用量（provider 实报）",
                Style::default().fg(accent(t)),
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
            Style::default().fg(accent(t)),
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

fn body_lines(app: &App, t: &Theme, width: u16) -> Vec<Line<'static>> {
    let Some(d) = &app.detail else {
        return vec![Line::styled(
            "等待监工写入状态…",
            Style::default().fg(muted(t)),
        )];
    };
    let mut lines = vec![Line::styled(
        "你",
        Style::default().fg(accent(t)).add_modifier(Modifier::BOLD),
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
            danger(t)
        } else {
            muted(t)
        }),
    ));
    if let Some(notice) = &d.record.notice {
        lines.push(Line::styled(
            clean(notice),
            Style::default().fg(warning(t)),
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
                Style::default().fg(warning(t)),
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
            Style::default().fg(muted(t)),
        ));
        for (_, name, state, summary) in tools {
            let color = match state.as_str() {
                "success" | "ok" => ok(t),
                "error" | "failed" | "denied" => danger(t),
                _ => muted(t),
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
            Style::default().fg(accent(t)),
        ));
        lines.push(Line::raw(format!("终态：{}", r.status.as_str())));
        for b in &r.blockers {
            lines.push(Line::styled(
                clean(&format!("{}: {}", b.code, b.message)),
                Style::default().fg(danger(t)),
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
                Style::default().fg(warning(t)),
            ));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled("模型的陈述", Style::default().fg(accent(t))));
        // Rendered to the body width here, under its heading; `fit_width` leaves rows that
        // already fit alone, so the hanging indents survive. (The first cut appended this at
        // the end of `body_lines`, which slid the truncation warning between heading and body.)
        lines.extend(statement_lines(app, t, width));
    } else if d.record.terminal {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "状态已终止，但结果文件不可读；请检查 /evidence",
            Style::default().fg(warning(t)),
        ));
    } else {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "等待最终正文；此通道没有实时 token 输出",
            Style::default().fg(muted(t)),
        ));
    }
    if d.truncated {
        lines.push(Line::styled(
            "显示内容已截断；完整内容见 /evidence",
            Style::default().fg(warning(t)),
        ));
    }
    lines
}

/// Assistant Markdown, or nothing when this run has no result file.
///
/// `final_text` is the model's own statement; `result.summary` is Arc Deck's fallback
/// for a worker that never wrote one. That is the same choice the old line-by-line
/// renderer made, kept so the fallback does not silently disappear with the rewrite.
fn statement_lines(app: &App, t: &Theme, width: u16) -> Vec<Line<'static>> {
    let Some(detail) = &app.detail else {
        return Vec::new();
    };
    let Some(result) = &detail.result else {
        return Vec::new();
    };
    let text = detail.final_text.as_deref().unwrap_or(&result.summary);
    markdown::render(text, width, &markdown_palette(t))
}

/// Maps the theme's semantic roles onto the Markdown palette. Nothing here reads the
/// other theme tokens on purpose: a Markdown role that borrowed an unrelated colour
/// would break the "状态不借用无关颜色" rule in PRODUCT_PLAN §7.3.
fn markdown_palette(t: &Theme) -> MarkdownPalette {
    // 主题的 markdown token 优先（OpenCode 主题里的 markdown* 一组）；缺的回退到正文色 ——
    // 2.0.27 的规则就是「未命中回落 text」，不去借 feedback 之类不相干的状态色。
    let text = t.text.over(t.background);
    let ink = |name: &str| t.markdown(name).map(|ink| ink.over(t.background)).unwrap_or(text);
    MarkdownPalette {
        text: ink("text"),
        muted: muted(t),
        heading: ink("heading"),
        link: ink("link"),
        link_text: ink("link_text"),
        code: ink("code"),
        code_block_fg: ink("code_block"),
        code_block_bg: panel_highlight(t),
        block_quote: ink("block_quote"),
        emph: ink("emph"),
        strong: ink("strong"),
        list_marker: ink("list_item"),
        hr: ink("horizontal_rule"),
    }
}

fn popup(frame: &mut Frame, app: &App, dialog: &Dialog, area: Rect, t: &Theme) {
    if matches!(dialog, Dialog::Theme) {
        if let Some(theme_dialog) = &app.theme_dialog {
            theme_dialog::draw(frame, theme_dialog, area, t);
        }
        return;
    }
    // 模型/provider/effort 三个对话框在 model_dialog 里，其余三个仍是这里的小浮层。
    match dialog {
        Dialog::Models(state) => {
            let search = model_dialog::draw_model(
                frame,
                area,
                t,
                model_dialog::ModelView {
                    providers: &app.boot.catalog,
                    prefs: &app.model_prefs,
                    state: *state,
                    query: &app.filter,
                    choice: app.choice,
                    current: app.model.as_ref(),
                },
            );
            frame.set_cursor_position((search.x + app.filter.chars().count() as u16, search.y));
            return;
        }
        Dialog::Providers(_) => {
            let search = model_dialog::draw_providers(
                frame,
                area,
                t,
                model_dialog::ProviderView {
                    providers: &app.boot.catalog,
                    query: &app.filter,
                    choice: app.choice,
                    current: app.model.as_ref(),
                },
            );
            frame.set_cursor_position((search.x + app.filter.chars().count() as u16, search.y));
            return;
        }
        Dialog::Efforts(state) => {
            model_dialog::draw_efforts(
                frame,
                area,
                t,
                model_dialog::EffortView {
                    providers: &app.boot.catalog,
                    state: Some(state),
                    choice: app.choice,
                    current: app.effort.as_deref(),
                },
            );
            return;
        }
        // 主题选择器在函数开头就交给 theme_dialog 画了。
        Dialog::Commands | Dialog::Runs | Dialog::Agents | Dialog::Theme => {}
    }
    // 40×12 之类的窄终端会给出一条负宽度的矩形；用 saturating_sub 而不是 `-`，
    // 否则 debug 构建直接 panic（「窗口太小」正是这条路径最常遇到的情形）。
    let w = 72.min(area.width.saturating_sub(2));
    let h = 18.min(area.height.saturating_sub(2)).max(4);
    let rect = Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    );
    let title = match dialog {
        Dialog::Commands => "命令",
        Dialog::Runs => "运行历史",
        Dialog::Agents => "Agent",
        Dialog::Theme => return,
        // 上面已经把这三个分支返回掉了；这里给编译器一个穷尽的分支。
        Dialog::Models(_) | Dialog::Providers(_) | Dialog::Efforts(_) => return,
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::bordered()
            .title(title)
            .border_style(Style::default().fg(t.border.to_ratatui()))
            .style(Style::default().bg(t.dialog_background(t.background_raised).over(t.background))),
        rect,
    );
    let inner = Rect::new(rect.x + 2, rect.y + 1, rect.width - 4, rect.height - 2);
    // 走到这里的只有 Commands / Runs / Agents 三个（上面已经 return 掉另外三个）。
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
        Dialog::Theme => return,
        Dialog::Models(_) | Dialog::Providers(_) | Dialog::Efforts(_) => return,
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
        Paragraph::new(search).style(Style::default().fg(muted(t))),
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
                let (fg, bg) = row_focus(t);
                Style::default()
                    .fg(if enabled {
                        fg
                    } else {
                        t.text_action(Variant::Primary, &[State::Disabled]).over(t.background)
                    })
                    .bg(if enabled { bg } else { panel_highlight(t) })
            } else {
                Style::default().fg(if enabled {
                    t.text.to_ratatui()
                } else {
                    t.text_action(Variant::Primary, &[State::Disabled]).over(t.background)
                })
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

/// `markdown::clean` under the old local name: every non-Markdown field in the
/// transcript is cleaned the same way the Markdown body is.
fn clean(text: &str) -> String {
    markdown::clean(text)
}

/// Last-resort width guard for `body_lines` output: only a row wider than the body
/// gets touched. Markdown renders itself to the width it is handed, so in practice
/// nothing here is re-wrapped — and that is the point. Re-wrapping Markdown output
/// unconditionally would destroy the hanging indents the renderer just built
/// (`fit_width` drops a line's prefix when it breaks it).
fn fit_width(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(2);
    let mut result = Vec::new();
    for line in lines {
        if line.width() <= width {
            result.push(line);
            continue;
        }
        result.extend(wrap(vec![line], width));
    }
    result
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
                catalog: vec![crate::catalog::Provider {
                    id: "opencode-go".into(),
                    name: "OpenCode Go".into(),
                    models: vec![
                        {
                            let mut with_effort = crate::catalog::Model {
                                id: "deepseek-v4.1-flash".into(),
                                name: Some("DeepSeek V4.1 Flash".into()),
                                ..crate::catalog::Model::default()
                            };
                            with_effort.efforts =
                                vec!["off".into(), "low".into(), "high".into(), "max".into()];
                            with_effort
                        },
                        crate::catalog::Model {
                            id: "glm-4.6".into(),
                            name: Some("GLM 4.6".into()),
                            ..crate::catalog::Model::default()
                        },
                    ],
                }],
                model_prefs: "unused.json".into(),
            },
            Preferences {
                sidebar: true,
                ..Preferences::default()
            },
            crate::catalog::ModelPrefs::default(),
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
                set_mode(&mut a, light);
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
                    Some(Dialog::Models(crate::model_dialog::ModelDialogState::default())),
                    Some(Dialog::Providers(
                        crate::model_dialog::ProviderDialogState::default(),
                    )),
                    Some(Dialog::Efforts(crate::model_dialog::EffortDialogState {
                        model: crate::catalog::parse_route("opencode-go/deepseek-v4.1-flash").unwrap(),
                        efforts: 4,
                    })),
                ] {
                    a.dialog = dialog;
                    a.theme_dialog = None;
                    terminal.draw(|f| draw(f, &mut a)).unwrap();
                }
                // The theme picker is rendered by its own module, so it gets its own pass here
                // rather than being folded into the list above.
                a.open_dialog(Dialog::Theme);
                terminal.draw(|f| draw(f, &mut a)).unwrap();
                let buffer = terminal.backend().buffer();
                let dump = row_text(buffer, 1);
                assert!(
                    contains_wrapped(buffer, "主题"),
                    "theme dialog title missing at {width}x{height}: {dump:?}"
                );
                assert!(
                    contains_wrapped(buffer, "opencode"),
                    "the default theme name is missing at {width}x{height}"
                );
                a.dialog = None;
                a.theme_dialog = None;
                assert_eq!(
                    terminal.backend().buffer().area,
                    Rect::new(0, 0, width, height)
                );
            }
        }
    }
    /// The app owns the resolved theme, so a test that wants a mode has to go through the same
    /// path the UI does rather than constructing a `Theme` directly.
    fn set_mode(app: &mut App, light: bool) {
        app.preferences.light = light;
        app.apply_preferences_theme();
    }
    /// Matches inside one rendered row. Cells are stepped by grapheme width, not by one, because
    /// a CJK glyph's second cell repeats the same symbol — reading cell by cell inserts a
    /// duplicate and makes `contains` miss ("主题" reads back as "主主题").
    fn contains_wrapped(buffer: &ratatui::buffer::Buffer, needle: &str) -> bool {
        (0..buffer.area.height).any(|y| row_text(buffer, y).contains(needle))
    }
    fn row_text(buffer: &ratatui::buffer::Buffer, y: u16) -> String {
        let mut row = String::new();
        let mut x = 0;
        while x < buffer.area.width {
            let glyph = buffer[(x, y)].symbol();
            row.push_str(glyph);
            x += (glyph.width() as u16).max(1);
        }
        row
    }
    #[test]
    fn model_dialogs_still_render_at_the_smallest_supported_terminal() {
        for (width, height) in [(40, 8), (40, 12), (60, 16)] {
            let mut a = app();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for dialog in [
                Dialog::Models(crate::model_dialog::ModelDialogState::default()),
                Dialog::Providers(crate::model_dialog::ProviderDialogState::default()),
                Dialog::Efforts(crate::model_dialog::EffortDialogState {
                    model: crate::catalog::parse_route("opencode-go/deepseek-v4.1-flash").unwrap(),
                    efforts: 4,
                }),
            ] {
                a.dialog = Some(dialog);
                // 不允许 panic，也不允许画到 buffer 之外。
                terminal.draw(|f| draw(f, &mut a)).unwrap();
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
        let text = body_lines(&a, &a.theme.clone(), 80)
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
    /// Kept from the hand-written palette era: normal text must never be painted with the
    /// failure colour, and the input surface must be distinguishable from the page. Both are
    /// still meaningful now that the colours come from OpenCode's tokens, and both would fail
    /// if a role were wired to the wrong token.
    #[test]
    fn semantic_themes_do_not_use_failure_color_for_normal_text() {
        for mode in [crate::theme::Mode::Dark, crate::theme::Mode::Light] {
            let t = crate::theme::Theme::load(crate::theme::DEFAULT_THEME, mode).unwrap();
            let body = t.text.to_ratatui();
            for kind in [
                crate::theme::Feedback::Error,
                crate::theme::Feedback::Warning,
                crate::theme::Feedback::Success,
            ] {
                assert_ne!(body, t.text_feedback(kind, false).over(t.background));
            }
            assert_ne!(t.background.to_ratatui(), muted(&t));
            // `muted` must stay a *text* role: the accent and the primary background are for
            // chrome, not for prose.
            assert_ne!(muted(&t), accent(&t));
            assert_ne!(
                t.text_feedback(crate::theme::Feedback::Error, false).over(t.background),
                t.text_feedback(crate::theme::Feedback::Success, false).over(t.background)
            );
        }
    }
    /// §7.3's "the picture must not get worse" is otherwise unfalsifiable. Each role the
    /// hand-written two-colour palette held is pinned to the value the 2.0.27 tokens resolve to
    /// for the default theme, so a re-wiring shows up as a number rather than as an impression.
    ///
    /// Three roles do not land on their exact old value, and all three are recorded here on
    /// purpose:
    /// - `background_element` `#282828` → `background.raised.high` `#1e1e1e`. `#282828` is
    ///   `darkStep4` of the v1 palette and is not a step of the v2 neutral scale; `raised.high`
    ///   is the nearest token with the same meaning ("one surface above the panel").
    /// - `action_disabled` `#606060` → `text.action.primary.$disabled` `#808080`.
    /// - `formfield` `#1e1e1e` → `background.formfield`, which is `$background.base` in every
    ///   built-in theme, so the composer loses its tint (see the note in `composer`).
    ///
    /// `feedback_warning` also moves (`#e5c07b` → `#f5a742`) because OpenCode's own default theme
    /// moved it, not because of the mapping.
    #[test]
    fn every_hand_written_role_lands_on_a_token() {
        use crate::theme::{DEFAULT_THEME, Mode, Theme};
        let t = Theme::load(DEFAULT_THEME, Mode::Dark).unwrap();
        let exact = [
            ("text", t.text.to_ratatui(), 0xeeeeee),
            ("text_muted", muted(&t), 0x808080),
            ("background", t.background.to_ratatui(), 0x0a0a0a),
            ("background_panel", panel_deep(&t), 0x141414),
            ("action", accent(&t), 0xfab283),
            ("border", t.border.to_ratatui(), 0x484848),
            ("feedback_error", danger(&t), 0xe06c75),
            ("feedback_success", ok(&t), 0x7fd88f),
            ("feedback_warning", warning(&t), 0xf5a742),
            // The three documented moves.
            ("background_element", panel_highlight(&t), 0x1e1e1e),
            (
                "action_disabled",
                t.text_action(Variant::Primary, &[State::Disabled]).over(t.background),
                0x808080,
            ),
            (
                "formfield",
                t.background_formfield(&[]).over(t.background),
                0x0a0a0a,
            ),
        ];
        for (role, actual, expected) in exact {
            let expected = Color::Rgb((expected >> 16) as u8, (expected >> 8) as u8, expected as u8);
            assert_eq!(actual, expected, "{role} moved");
        }
    }

    /// The highlighted row must be legible in **every** built-in theme: this is the pair
    /// `DialogSelect` paints, and getting it wrong is invisible-looking (same colour on both
    /// sides) rather than obviously broken.
    #[test]
    fn default_themes_paint_the_highlighted_row_legibly() {
        for name in crate::theme::theme_names() {
            for mode in [crate::theme::Mode::Dark, crate::theme::Mode::Light] {
                let t = crate::theme::Theme::load(name, mode).unwrap();
                let (fg, bg) = row_focus(&t);
                assert_ne!(fg, bg, "{name} ({mode:?}) paints the highlighted row invisibly");
            }
        }
    }

    /// The picker previews a theme the moment the cursor lands on it, so every built-in theme has
    /// to survive a full draw in both modes. Resolving successfully is not enough: a theme that
    /// panics while painting would blank the screen mid-browse.
    #[test]
    fn every_built_in_theme_draws_the_picker_in_both_modes() {
        for name in crate::theme::theme_names() {
            for mode in [crate::theme::Mode::Dark, crate::theme::Mode::Light] {
                let mut a = app();
                a.preferences.theme = Some(name.to_string());
                set_mode(&mut a, mode == crate::theme::Mode::Light);
                a.open_dialog(Dialog::Theme);
                let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
                terminal.draw(|f| draw(f, &mut a)).unwrap();
                assert!(
                    contains_wrapped(terminal.backend().buffer(), "主题"),
                    "{name} ({mode:?}) did not paint the picker"
                );
            }
        }
    }

    #[test]
    fn generate_reviewable_cell_snapshots() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tui-preview");
        std::fs::create_dir_all(&dir).unwrap();
        for (w, h) in [(40, 12), (80, 24), (120, 30), (160, 40)] {
            for light in [false, true] {
                for screen in ["home", "run", "commands", "theme"] {
                    let mut a = app();
                    set_mode(&mut a, light);
                    if screen != "home" {
                        long_run(&mut a);
                    }
                    if screen == "commands" {
                        a.open_dialog(Dialog::Commands);
                    }
                    if screen == "theme" {
                        a.open_dialog(Dialog::Theme);
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
