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

use arc_deck_core::conversation::{
    AssistantTurn, Block as ViewBlock, Message, ToolCall, ToolStatus, TurnState,
};

use crate::{
    app::{App, Dialog, token},
    commands::{self, Action, Category},
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
    let leader = app.keymap.leader_label();
    let hint = if app.leader_active() {
        // Which-key, generated from the same keymap that resolves the chord: the strip cannot
        // advertise a combination that no longer exists after a rebind.
        let mut text = format!("{leader}：");
        for (index, (key, title)) in app.leader_hints().into_iter().enumerate() {
            if index > 0 {
                text.push_str(" · ");
            }
            text.push_str(&format!("{key} {title}"));
        }
        text
    } else if cfg!(windows) && area.width < 65 {
        "F6发送 Enter换行 ^P命令 F1帮助 ^C退出".into()
    } else if cfg!(windows) {
        format!(
            "F6 发送 · Enter 换行 · ^P 命令 · {leader} leader · F1 帮助 · PgUp 上滚 · Esc 取消 · /exit 退出"
        )
    } else if area.width < 65 {
        "Enter发送 ^J换行 ^P命令 F1帮助 ^C退出".into()
    } else {
        format!(
            "Enter 发送 · ^J 换行 · ^P 命令 · {leader} leader · F1 帮助 · PgUp 上滚 · Esc 取消 · ^C 退出"
        )
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(clean(&app.notice), Style::default().fg(warning(t))),
            Line::styled(hint, Style::default().fg(muted(t))),
        ]),
        footer,
    );
    let content = Rect::new(area.x, area.y, area.width, area.height - 2);
    let composer_area = if app.selected.is_none() && !app.in_conversation() {
        home(frame, app, content, t)
    } else {
        session(frame, app, content, t)
    };
    if app.sidebar_overlay
        && app.preferences.sidebar
        && content.width <= 120
        && (app.selected.is_some() || app.in_conversation())
    {
        let w = 42.min(content.width - 2);
        sidebar(
            frame,
            app,
            Rect::new(content.right() - w, content.y, w, content.height),
            t,
        );
    }
    // The popup is anchored to the composer, not the screen: on Home the composer sits in the
    // middle, and a completion list glued to the bottom edge would look like a different widget.
    if let Some(dialog) = &app.dialog {
        popup(frame, app, dialog, content, t);
    } else if app.autocomplete.is_some() {
        autocomplete_popup(frame, app, composer_area, content, t);
    }
}

fn home(frame: &mut Frame, app: &App, area: Rect, t: &Theme) -> Rect {
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
        Paragraph::new(home_subtitle(app)).style(Style::default().fg(muted(t))),
        Rect::new(x, y + 2, width, 1),
    );
    let composer_area = Rect::new(x, y + 3, width, height.saturating_sub(3));
    composer(frame, app, composer_area, t);
    if area.height > 16 {
        frame.render_widget(
            Paragraph::new(clean(&app.boot.workspace.display().to_string()))
                .style(Style::default().fg(muted(t))),
            Rect::new(x, y + height + 1, width, 1),
        );
    }
    composer_area
}

fn session(frame: &mut Frame, app: &mut App, area: Rect, t: &Theme) -> Rect {
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
    let label = if app.in_conversation() {
        app.conversation
            .as_ref()
            .and_then(|c| c.title.as_deref())
            .unwrap_or("新会话")
    } else {
        app.detail
            .as_ref()
            .map(|d| d.record.title.as_str())
            .unwrap_or("正在接入运行记录…")
    };
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
    let lines = if app.in_conversation() {
        conversation_lines(app, t, body.width)
    } else {
        body_lines(app, t, body.width)
    };
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
    let composer_area = Rect::new(
        main.x + 1,
        main.bottom() - composer_height,
        main.width - 2,
        composer_height,
    );
    composer(frame, app, composer_area, t);
    composer_area
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
        vec![composer_placeholder(app).into()]
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
    // 元数据按**省略顺序**拼，而不是按宽度阈值：模型优先级最高（它决定这一轮到底发给谁），
    // 然后 agent、effort、授权级别。宽度不够时从优先级最低的一项开始丢 —— 固定阈值会在某个
    // 宽度上同时跳掉两项，看起来像显示坏了。
    let mut parts: Vec<(u8, String)> = vec![(0, app.model_name()), (1, app.agent.clone())];
    if let Some(effort) = &app.effort {
        parts.push((2, format!("effort {effort}")));
    }
    parts.push((3, app.permission.clone()));
    loop {
        let joined = parts
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join(" · ");
        if joined.width() <= inner.width as usize || parts.len() <= 1 {
            break;
        }
        let Some(index) = parts
            .iter()
            .enumerate()
            .max_by_key(|(_, (priority, _))| *priority)
            .map(|(index, _)| index)
        else {
            break;
        };
        parts.remove(index);
    }
    let metadata = parts
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    frame.render_widget(
        Paragraph::new(metadata).style(Style::default().fg(accent(t))),
        Rect::new(inner.x, area.bottom() - 2, inner.width, 1),
    );
    let overlay = app.sidebar_overlay
        && app.preferences.sidebar
        && frame.area().width <= 120
        && (app.selected.is_some() || app.in_conversation());
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
    if app.in_conversation() {
        lines.extend(conversation_sidebar(app, t));
    } else if let Some(d) = &app.detail {
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
            // 一次运行时 stderr 可能有 32 KiB / 上百行（Node 的栈），整段画进正文会把
            // 真正的对话推出屏幕。只显示第一条（通常是摘要），其余指向 /evidence ——
            // 诊断仍然可见，只是不再占满屏幕。
            let message = str_at("message");
            let mut parts = message.lines().filter(|line| !line.trim().is_empty());
            let first = parts.next().unwrap_or("");
            let rest = parts.count();
            lines.push(Line::styled(clean(first), Style::default().fg(warning(t))));
            if rest > 0 {
                lines.push(Line::styled(
                    format!("…（另有 {rest} 行诊断，完整内容见 /evidence）"),
                    Style::default().fg(muted(t)),
                ));
            }
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
    // 模型/provider/effort 三个对话框在 model_dialog 里；命令面板与帮助各有自己的行模型，
    // 剩下三个（运行历史 / Agent / 授权级别）共用下面的小浮层。
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
        Dialog::Commands => {
            commands_popup(frame, app, area, t);
            return;
        }
        Dialog::Help => {
            help_popup(frame, app, area, t);
            return;
        }
        // 主题选择器在函数开头就交给 theme_dialog 画了。
        Dialog::Sessions | Dialog::Runs | Dialog::Agents | Dialog::Permissions | Dialog::Theme => {}
    }
    let rect = dialog_rect(area);
    let title = match dialog {
        Dialog::Sessions => "会话（当前工作区）",
        Dialog::Runs => "工人运行（当前工作区）",
        Dialog::Agents => "Agent",
        Dialog::Permissions => "授权级别",
        Dialog::Theme => return,
        // 上面已经把其余分支返回掉了；这里给编译器一个穷尽的分支。
        Dialog::Commands
        | Dialog::Help
        | Dialog::Models(_)
        | Dialog::Providers(_)
        | Dialog::Efforts(_) => return,
    };
    let inner = dialog_frame(frame, rect, title, t);
    // 走到这里的只有 Runs / Agents / Permissions 三个。
    let mut rows: Vec<(String, bool)> = match dialog {
        Dialog::Sessions => {
            let now = now_ms();
            app.filtered_sessions()
                .iter()
                .map(|s| {
                    let title = s.title.clone().unwrap_or_else(|| "（无标题）".into());
                    let running = if s.running { " · 进行中" } else { "" };
                    let current = if app.session_id.as_deref() == Some(s.session_id.as_str()) {
                        "（当前）"
                    } else {
                        ""
                    };
                    (format!("{title}{current} · {}{running}", ago(now, s.updated_at)), true)
                })
                .collect()
        }
        Dialog::Runs => app
            .filtered_runs()
            .iter()
            .map(|r| (format!("{} · {} · {}", r.title, r.state, r.health), true))
            .collect(),
        Dialog::Agents => vec![
            ("Echo · 默认工程角色".into(), true),
            ("Build · 实施".into(), true),
            (
                format!(
                    "Plan · 不可用：{}",
                    app.availability(Action::Plan).unwrap_or_default()
                ),
                false,
            ),
        ],
        Dialog::Permissions => commands::PERMISSIONS
            .iter()
            .map(|(name, description, reason)| match reason {
                // Unavailable rows stay visible with the reason: hiding readonly would make
                // "why can't I pick it" unanswerable from inside the app.
                Some(reason) => (format!("{name} · {description} — {reason}"), false),
                None => {
                    let current = if *name == app.permission { "（当前）" } else { "" };
                    (format!("{name} · {description}{current}"), true)
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    if rows.is_empty() {
        rows.push(("没有匹配项".into(), false));
    }
    let search = if matches!(dialog, Dialog::Runs | Dialog::Sessions) {
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

/// Shared popup frame: 40×12 之类的窄终端会给出一条负宽度的矩形；用 saturating_sub 而不是
/// `-`，否则 debug 构建直接 panic（「窗口太小」正是这条路径最常遇到的情形）。
fn dialog_rect(area: Rect) -> Rect {
    let w = 72.min(area.width.saturating_sub(2));
    let h = 18.min(area.height.saturating_sub(2)).max(4);
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

/// The palette and help are as long as the registry; a fixed 18 rows would hide whole
/// categories (and their reasons) with no indication that there is more below.
fn dialog_rect_tall(area: Rect) -> Rect {
    let w = 72.min(area.width.saturating_sub(2));
    let h = area.height.saturating_sub(2).max(4);
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    )
}

fn dialog_frame(frame: &mut Frame, rect: Rect, title: &str, t: &Theme) -> Rect {
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::bordered()
            .title(title)
            .border_style(Style::default().fg(t.border.to_ratatui()))
            .style(Style::default().bg(t.dialog_background(t.background_raised).over(t.background))),
        rect,
    );
    Rect::new(rect.x + 2, rect.y + 1, rect.width - 4, rect.height - 2)
}

/// Command palette. Rows are grouped by category and carry the keybinding the keymap really
/// resolves; an action the backend cannot serve stays listed, greyed, and its reason is shown
/// on the footer line when it is the selected row.
fn commands_popup(frame: &mut Frame, app: &App, area: Rect, t: &Theme) {
    let rect = dialog_rect_tall(area);
    let inner = dialog_frame(frame, rect, "命令", t);
    frame.render_widget(
        Paragraph::new(format!("搜索：{}", app.filter)).style(Style::default().fg(muted(t))),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    enum Row<'a> {
        Header(&'static str),
        Item(&'a commands::Entry, Option<String>),
    }
    let entries = app.filtered_commands();
    let mut rows: Vec<Row> = Vec::new();
    let mut category: Option<Category> = None;
    for entry in &entries {
        if category != Some(entry.category) {
            category = Some(entry.category);
            rows.push(Row::Header(entry.category.label()));
        }
        let reason = entry
            .unavailable
            .map(str::to_string)
            .or_else(|| app.availability(entry.action));
        rows.push(Row::Item(entry, reason));
    }
    if entries.is_empty() {
        rows.push(Row::Header("没有匹配项"));
    }
    let selected = entries.get(app.choice).map(|entry| entry.action);
    let selected_row = rows
        .iter()
        .position(|row| matches!(row, Row::Item(entry, _) if Some(entry.action) == selected));
    // One row for the search line, one for the footer; the rest is the list.
    let visible = inner.height.saturating_sub(2) as usize;
    let start = selected_row
        .unwrap_or(0)
        .saturating_sub(visible.saturating_sub(1));
    let width = inner.width as usize;
    let mut lines = Vec::new();
    for row in rows.iter().skip(start).take(visible) {
        match row {
            Row::Header(title) => lines.push(Line::styled(
                clean(&format!("— {title}")),
                Style::default()
                    .fg(muted(t))
                    .add_modifier(Modifier::BOLD),
            )),
            Row::Item(entry, reason) => {
                let is_selected = Some(entry.action) == selected;
                let available = reason.is_none();
                let marker = if is_selected { "›" } else { " " };
                let slash = entry.slash.unwrap_or("");
                let keys = app.keymap.display(entry.action).unwrap_or_default();
                let mut text = format!("{marker} {slash:<14}{}", entry.title);
                let used = text.width() + keys.width();
                if !keys.is_empty() && used < width {
                    text.push_str(&" ".repeat(width - used));
                    text.push_str(&keys);
                }
                let fg = if !available {
                    t.text_action(Variant::Primary, &[State::Disabled]).over(t.background)
                } else if is_selected {
                    row_focus(t).0
                } else {
                    t.text.to_ratatui()
                };
                let style = Style::default().fg(fg);
                let style = if is_selected && available {
                    style.bg(row_focus(t).1)
                } else {
                    style
                };
                lines.push(Line::styled(clean(&text), style));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(2),
        ),
    );
    let footer = match selected.and_then(|action| app.availability(action)) {
        Some(reason) => reason,
        None => "↑↓ 选择 · Enter 执行 · Esc 返回".into(),
    };
    frame.render_widget(
        Paragraph::new(clean(&footer)).style(Style::default().fg(muted(t))),
        Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            1,
        ),
    );
}

/// Help lists every binding by category. The reason for an unavailable action is part of the
/// row, not a tooltip: terminals have no hover.
fn help_popup(frame: &mut Frame, app: &App, area: Rect, t: &Theme) {
    let rect = dialog_rect_tall(area);
    let inner = dialog_frame(frame, rect, "帮助", t);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut category: Option<Category> = None;
    for row in app.help_rows() {
        if category != Some(row.category) {
            category = Some(row.category);
            lines.push(Line::styled(
                clean(row.category.label()),
                Style::default()
                    .fg(accent(t))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        let keys = format!("{:<18}", row.keys);
        // Context is part of the documentation: Ctrl+F means "favourite" only inside the
        // model dialog, and a help page that omits that is worse than no help page.
        let context = match row.context {
            commands::Context::ModelDialog => "（仅模型对话框）",
            commands::Context::Anywhere | commands::Context::Composer => "",
        };
        let detail = match row.reason {
            Some(reason) => format!("{context}{} — {reason}", row.title),
            None => format!("{context}{}", row.title),
        };
        lines.push(Line::from(vec![
            Span::styled(clean(&keys), Style::default().fg(muted(t))),
            Span::raw(clean(&detail)),
        ]));
    }
    // Only the footer row is reserved; at 120×40 the whole reference must fit without
    // scrolling, because Ctrl+P lives on the very last row.
    let visible = inner.height.saturating_sub(1) as usize;
    let start = app.help_scroll.min(lines.len().saturating_sub(1));
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(start)
                .take(visible)
                .collect::<Vec<_>>(),
        )
        .wrap(ratatui::widgets::Wrap { trim: true }),
        Rect::new(inner.x, inner.y, inner.width, visible as u16),
    );
    frame.render_widget(
        Paragraph::new("↑↓ 滚动 · Enter / Esc 关闭").style(Style::default().fg(muted(t))),
        Rect::new(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            inner.width,
            1,
        ),
    );
}

/// `/` completion. Anchored above the composer so it does not cover the draft it is completing.
fn autocomplete_popup(frame: &mut Frame, app: &App, composer: Rect, area: Rect, t: &Theme) {
    let Some(popup) = &app.autocomplete else {
        return;
    };
    let rows = popup.items.len().min(7);
    let width = 64.min(area.width.saturating_sub(2)).max(20);
    let height = (rows as u16 + 2).min(area.height.saturating_sub(2)).max(3);
    let x = composer
        .x
        .min(area.right().saturating_sub(width))
        .max(area.x);
    let y = composer.y.saturating_sub(height).max(area.y);
    let rect = Rect::new(x, y, width, height);
    let title = if popup.trigger == '@' {
        "@ 文件引用"
    } else {
        "命令补全"
    };
    let inner = dialog_frame(frame, rect, title, t);
    let mut lines = Vec::new();
    for (index, item) in popup.items.iter().enumerate().take(rows) {
        let selected = index == popup.selected;
        let fg = if !item.available {
            t.text_action(Variant::Primary, &[State::Disabled]).over(t.background)
        } else if selected {
            row_focus(t).0
        } else {
            t.text.to_ratatui()
        };
        // Slash rows show the command and then its title; file rows have no second label, and
        // their detail *is* the path, so printing the insert text again would duplicate it.
        let left = match item.action {
            Some(_) => item.insert.trim_end(),
            None => item.detail.as_str(),
        };
        let mut text = format!(
            "{} {:<14}{}",
            if selected { "›" } else { " " },
            left,
            item.title
        );
        if !item.available && text.width() + 6 <= inner.width as usize {
            text.push_str(" · 不可用");
        }
        let style = if selected && item.available {
            Style::default().fg(fg).bg(row_focus(t).1)
        } else {
            Style::default().fg(fg)
        };
        lines.push(Line::styled(clean(&text), style));
    }
    frame.render_widget(Paragraph::new(lines), inner);
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

fn home_subtitle(app: &App) -> &'static str {
    use arc_deck_core::service::BackendState;
    match app.backend {
        BackendState::Ready { .. } => "从当前项目开始一个会话",
        BackendState::Starting => "从当前项目开始一个会话（交互内核启动中）",
        _ => "交互会话不可用：每次提交是一次独立请求",
    }
}

fn composer_placeholder(app: &App) -> &'static str {
    use arc_deck_core::service::BackendState;
    if !matches!(app.backend, BackendState::Ready { .. } | BackendState::Starting) {
        "输入指令…每次提交创建独立运行"
    } else if app.conversation.as_ref().is_some_and(|c| c.running) {
        "输入下一条…这一轮结束后发送"
    } else if app.session_id.is_some() {
        "继续这个会话…"
    } else {
        "输入消息开始新会话…"
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 相对时间。会话列表里人要的是「哪一个是我昨天那个」，不是精确时刻。
fn ago(now: u64, then: u64) -> String {
    if then == 0 || then > now {
        return "时间未知".into();
    }
    let minutes = (now - then) / 60_000;
    match minutes {
        0 => "刚刚".into(),
        1..=59 => format!("{minutes} 分钟前"),
        60..=1439 => format!("{} 小时前", minutes / 60),
        _ => format!("{} 天前", minutes / 1440),
    }
}

fn conversation_sidebar(app: &App, t: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let Some(c) = &app.conversation else {
        lines.push(Line::styled("正在新建会话…", Style::default().fg(muted(t))));
        return lines;
    };
    let model = c
        .model
        .as_ref()
        .map(|m| match &m.effort {
            Some(effort) => format!("{}/{} · effort {effort}", m.provider, m.model),
            None => format!("{}/{}", m.provider, m.model),
        })
        .unwrap_or_else(|| "内核未报告".into());
    let state = if c.running { "进行中" } else { "空闲" };
    for (name, value) in [
        ("会话", c.title.clone().unwrap_or_else(|| "（无标题）".into())),
        ("ID", c.session_id.clone()),
        ("下一轮模型", model),
        ("状态", state.to_string()),
    ] {
        lines.push(Line::styled(name, Style::default().fg(muted(t))));
        lines.push(Line::raw(clean(&value)));
    }
    let mut total = arc_deck_core::conversation::TokenUsage::default();
    let mut turns = 0;
    for message in &c.messages {
        if let Message::Assistant(turn) = message {
            turns += 1;
            if let Some(usage) = &turn.usage {
                total.add(usage);
            }
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(format!("{turns} 轮"), Style::default().fg(muted(t))));
    // 只显示 provider 真的报告过的数（§7.3：信息栏只显示真实数据）。
    if total.input.is_some() || total.output.is_some() {
        lines.push(Line::styled("用量（provider 实报）", Style::default().fg(accent(t))));
        lines.push(Line::raw(format!("输入 {} · 输出 {}", token(total.input), token(total.output))));
        if total.cache_read.is_some() {
            lines.push(Line::raw(format!("缓存 {}", token(total.cache_read))));
        }
    }
    if !c.history_complete {
        lines.push(Line::styled("历史未取全", Style::default().fg(warning(t))));
    }
    lines
}

/// 会话正文。
///
/// 折叠规则（§7.3）：思考与工具输出默认折叠，`/tools` 展开；**失败与被拒在折叠态也看得见** ——
/// 折叠是为了少占屏幕，不是为了把出错藏起来。
fn conversation_lines(app: &App, t: &Theme, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let expanded = app
        .expanded
        .get(&format!("session:{}", app.session_id.clone().unwrap_or_default()))
        .copied()
        .unwrap_or(false);
    let you = || Line::styled("你", Style::default().fg(accent(t)).add_modifier(Modifier::BOLD));
    if let Some(c) = &app.conversation {
        if !c.history_complete {
            lines.push(Line::styled("（更早的历史没有取全）", Style::default().fg(warning(t))));
        }
        for message in &c.messages {
            match message {
                Message::User(user) => {
                    lines.push(you());
                    lines.extend(clean(&user.text).lines().map(|s| Line::raw(s.to_string())));
                    for attachment in &user.attachments {
                        lines.push(Line::styled(clean(attachment), Style::default().fg(muted(t))));
                    }
                    lines.push(Line::raw(""));
                }
                Message::Assistant(turn) => {
                    lines.extend(assistant_lines(turn, t, width, expanded));
                    lines.push(Line::raw(""));
                }
                Message::Unknown => {}
            }
        }
    }
    if let Some(text) = &app.pending {
        lines.push(you());
        lines.extend(clean(text).lines().map(|s| Line::raw(s.to_string())));
        lines.push(Line::styled("发送中…", Style::default().fg(muted(t))));
    }
    if lines.is_empty() {
        lines.push(Line::styled("正在打开会话…", Style::default().fg(muted(t))));
    }
    lines
}

fn assistant_lines(turn: &AssistantTurn, t: &Theme, width: u16, expanded: bool) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        "Arc Deck",
        Style::default().fg(muted(t)).add_modifier(Modifier::BOLD),
    )];
    for block in &turn.blocks {
        match block {
            ViewBlock::Text(part) => {
                lines.extend(markdown::render(&part.text, width, &markdown_palette(t)));
                if part.streaming {
                    lines.push(Line::styled("▍", Style::default().fg(accent(t))));
                }
            }
            ViewBlock::Reasoning(part) => {
                let chars = part.text.chars().count();
                if expanded {
                    lines.push(Line::styled("思考", Style::default().fg(muted(t))));
                    lines.extend(clean(&part.text).lines().map(|s| {
                        Line::styled(format!("  {s}"), Style::default().fg(muted(t)).add_modifier(Modifier::ITALIC))
                    }));
                } else {
                    let tail = if part.streaming { " · 进行中" } else { "" };
                    lines.push(Line::styled(
                        format!("思考 · {chars} 字{tail}（/tools 展开）"),
                        Style::default().fg(muted(t)),
                    ));
                }
            }
            ViewBlock::Tool(call) => lines.extend(tool_lines(call, t, expanded)),
            ViewBlock::Unknown => {}
        }
    }
    match &turn.state {
        TurnState::Running => {
            if turn.blocks.is_empty() {
                lines.push(Line::styled("正在思考…", Style::default().fg(muted(t))));
            }
        }
        TurnState::Completed => {}
        TurnState::Cancelled => lines.push(Line::styled("已取消", Style::default().fg(warning(t)))),
        TurnState::Failed { message, code } => {
            let code = code.as_deref().map(|c| format!("[{c}] ")).unwrap_or_default();
            lines.push(Line::styled(
                clean(&format!("模型失败：{code}{message}")),
                Style::default().fg(danger(t)),
            ));
        }
        TurnState::MaxTokens => lines.push(Line::styled("达到输出上限", Style::default().fg(warning(t)))),
        TurnState::Blocked => lines.push(Line::styled("这一轮被阻塞", Style::default().fg(warning(t)))),
        TurnState::Interrupted => {
            lines.push(Line::styled("这一轮在内核崩溃时中断", Style::default().fg(warning(t))))
        }
        TurnState::Other { kind } => {
            lines.push(Line::styled(clean(&format!("结束：{kind}")), Style::default().fg(muted(t))))
        }
        TurnState::Unknown => {}
    }
    if turn.retries > 0 && turn.state == TurnState::Completed {
        lines.push(Line::styled(
            format!("（模型调用失败后重试过 {} 次）", turn.retries),
            Style::default().fg(muted(t)),
        ));
    }
    lines
}

/// 一次工具调用：标题行 + 按需的输出。
fn tool_lines(call: &ToolCall, t: &Theme, expanded: bool) -> Vec<Line<'static>> {
    let subject = ["command", "path", "file_path", "pattern", "url", "query"]
        .iter()
        .find_map(|key| call.argument(key))
        .map(|value| format!(" {}", first_row(&value, 80)))
        .unwrap_or_default();
    let (mark, color, label) = match call.status {
        ToolStatus::Running => ("…", muted(t), "进行中"),
        ToolStatus::Done if call.exit_code.is_some_and(|c| c != 0) => ("✗", danger(t), "非零退出"),
        ToolStatus::Done => ("✓", ok(t), "完成"),
        ToolStatus::Failed => ("✗", danger(t), "失败"),
        ToolStatus::Abandoned => ("·", warning(t), "未完成"),
        ToolStatus::Unknown => ("?", muted(t), "未知"),
    };
    let exit = call.exit_code.map(|c| format!(" · exit {c}")).unwrap_or_default();
    let mut lines = vec![Line::styled(
        clean(&format!("  {mark} {}{subject} · {label}{exit}", call.name)),
        Style::default().fg(color),
    )];
    let failed = matches!(call.status, ToolStatus::Failed) || call.exit_code.is_some_and(|c| c != 0);
    if let Some(error) = &call.error {
        lines.push(Line::styled(clean(&format!("    {error}")), Style::default().fg(danger(t))));
    }
    if let Some(output) = &call.output {
        // 展开时给足；折叠时失败的那一条仍露出开头几行（拒因通常就在第一行）。
        let limit = if expanded { 40 } else if failed { 4 } else { 0 };
        let rows: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
        for row in rows.iter().take(limit) {
            lines.push(Line::styled(clean(&format!("    {row}")), Style::default().fg(muted(t))));
        }
        if limit > 0 && rows.len() > limit {
            lines.push(Line::styled(
                format!("    …另有 {} 行", rows.len() - limit),
                Style::default().fg(muted(t)),
            ));
        }
    }
    lines
}

fn first_row(text: &str, max: usize) -> String {
    let row = text.lines().next().unwrap_or("");
    if row.chars().count() > max {
        format!("{}…", row.chars().take(max).collect::<String>())
    } else {
        row.to_string()
    }
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
                    Some(Dialog::Help),
                    Some(Dialog::Permissions),
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

    /// 运行时 stderr 的一次警告可能带整段 Node 栈；正文只该显示第一条摘要，
    /// 否则一屏对话被诊断顶掉（实测：0.2.1 的 libuv 退出断言夹着 5 行以上栈）。
    #[test]
    fn a_noisy_diagnostic_is_folded_to_its_first_line() {
        let mut a = app();
        long_run(&mut a);
        let detail = a.detail.as_mut().unwrap();
        detail.events.push(EventEnvelope::diagnostic(
            "r",
            9,
            "warn",
            None,
            "运行时 stderr：dsh: warning: 1 entry did not activate spill-local\n    at privateRoot (x)\nAssertion failed: !(handle->flags & UV_HANDLE_CLOSING)",
        ));
        let text = body_lines(&a, &a.theme.clone(), 100)
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("运行时 stderr"));
        assert!(text.contains("另有 2 行"));
        assert!(!text.contains("Assertion failed"), "the stack must not fill the screen");
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

    /// The palette used to be a flat list of `slash  label` rows. §7.3 asks for categories and
    /// keys, and an unavailable row has to carry its reason somewhere the user can read.
    #[test]
    fn the_palette_groups_by_category_shows_keys_and_explains_unavailable_rows() {
        let mut a = app();
        a.open_dialog(Dialog::Commands);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let buffer = terminal.backend().buffer();
        for needle in ["会话", "Agent 与模型", "界面", "帮助", "Ctrl+X N", "Ctrl+X T"] {
            assert!(
                contains_wrapped(buffer, needle),
                "the palette is missing {needle:?}: {:?}",
                row_text(buffer, 4)
            );
        }
        // Selecting the unavailable Plan row puts its reason on the footer line.
        a.filter = "plan".into();
        a.choice = 0;
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        assert!(
            contains_wrapped(terminal.backend().buffer(), "只读"),
            "the reason for the unavailable row must be visible: {:?}",
            row_text(terminal.backend().buffer(), 20)
        );
    }

    /// Help is generated from the registry + keymap; if either drifts, this fails rather than
    /// the page quietly documenting keys that do nothing.
    #[test]
    fn help_lists_the_real_bindings() {
        let mut a = app();
        a.open_dialog(Dialog::Help);
        let mut terminal = Terminal::new(TestBackend::new(120, 50)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let buffer = terminal.backend().buffer();
        for needle in ["Ctrl+C", "Shift+Tab", "Ctrl+P", "Plan", "只读", "仅模型对话框"] {
            assert!(
                contains_wrapped(buffer, needle),
                "help is missing {needle:?}"
            );
        }
    }

    /// The completion list has to sit above the composer and name the command it will insert.
    #[test]
    fn the_completion_popup_renders_above_the_composer() {
        let mut a = app();
        let (tx, _rx) = std::sync::mpsc::channel();
        a.input(
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('/'),
                crossterm::event::KeyModifiers::NONE,
            )),
            &tx,
        );
        assert!(a.autocomplete.is_some());
        for c in "the".chars() {
            a.input(
                crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(c),
                    crossterm::event::KeyModifiers::NONE,
                )),
                &tx,
            );
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let buffer = terminal.backend().buffer();
        assert!(contains_wrapped(buffer, "命令补全"));
        assert!(contains_wrapped(buffer, "/theme"));
    }

    /// File completion shares the popup but has its own title and shows the path exactly once.
    #[test]
    fn the_file_completion_popup_shows_the_path_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "x").unwrap();
        let mut a = app();
        a.boot.workspace = dir.path().to_path_buf();
        let (tx, _rx) = std::sync::mpsc::channel();
        for c in "看 @ma".chars() {
            a.input(
                crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char(c),
                    crossterm::event::KeyModifiers::NONE,
                )),
                &tx,
            );
        }
        assert!(a.autocomplete.is_some());
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let buffer = terminal.backend().buffer();
        assert!(contains_wrapped(buffer, "@ 文件引用"));
        let rows = (0..buffer.area.height)
            .filter(|y| row_text(buffer, *y).contains("@main.rs"))
            .count();
        assert_eq!(rows, 1, "the path must not be printed twice in one row");
    }

    /// Which-key: the footer lists the chords that can follow the leader. Generated from the
    /// keymap, so a rebind changes the strip too.
    #[test]
    fn the_leader_strip_lists_what_can_follow() {
        let mut a = app();
        let (tx, _rx) = std::sync::mpsc::channel();
        a.input(
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('x'),
                crossterm::event::KeyModifiers::CONTROL,
            )),
            &tx,
        );
        assert!(a.leader_active());
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let buffer = terminal.backend().buffer();
        assert!(contains_wrapped(buffer, "Ctrl+X："));
        assert!(contains_wrapped(buffer, "新会话"));
    }

    /// 元数据的省略顺序：模型最高、permission 最低。窄终端上必须仍看得见模型，
    /// 而不是四个字段一起消失或按固定阈值一下跳掉两项。
    #[test]
    fn metadata_drops_low_priority_parts_as_the_terminal_narrows() {
        for (width, height, workspace, agent) in [
            (120, 30, true, true),
            (40, 12, false, true),
            (30, 12, false, false),
        ] {
            let mut a = app();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| draw(f, &mut a)).unwrap();
            let buffer = terminal.backend().buffer();
            assert!(
                contains_wrapped(buffer, "DeepSeek V4.1 Flash"),
                "the model name is never dropped ({width}x{height})"
            );
            assert_eq!(
                contains_wrapped(buffer, "workspace"),
                workspace,
                "permission is the first to go ({width}x{height})"
            );
            assert_eq!(
                contains_wrapped(buffer, "echo"),
                agent,
                "agent goes after permission ({width}x{height})"
            );
        }
    }

    #[test]
    fn generate_reviewable_cell_snapshots() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tui-preview");
        std::fs::create_dir_all(&dir).unwrap();
        for (w, h) in [(40, 12), (80, 24), (120, 30), (160, 40)] {
            for light in [false, true] {
                for screen in ["home", "run", "commands", "theme", "help", "permissions"] {
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
                    if screen == "help" {
                        a.open_dialog(Dialog::Help);
                    }
                    if screen == "permissions" {
                        a.open_dialog(Dialog::Permissions);
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
