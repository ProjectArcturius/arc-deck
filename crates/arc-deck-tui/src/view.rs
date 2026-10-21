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
    widgets::{Block, Clear, Paragraph},
};
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use arc_deck_core::conversation::{
    ApprovalRequest, AssistantTurn, Block as ViewBlock, Message, ToolCall, ToolStatus, TurnState,
};

use crate::{
    app::{App, ConfirmKind, Dialog, Viewer, token},
    commands::{self, Category},
    markdown::{self, MarkdownPalette},
    model_dialog,
    theme::{Feedback, State, Theme, Variant},
    theme_dialog,
    toolview,
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

/// 每个 Agent 一种颜色（照 OpenCode `local.agent.color`：取色阶的 200 那一步）。
/// Echo 青、Build 橙、Plan 紫（用户 2026-10-20 定 Echo 为青色，其余由我们定）。
///
/// 内置主题的色阶并不齐（`aura` 没有 cyan、`osaka-jade` 只有 green/red/yellow —— 实测 35 个主题
/// 的色阶名单），所以每个 Agent 一条**偏好链**，按顺序取第一条存在、且没被前面的 Agent 用掉的；
/// 都没有时退到 categorical 里剩下的，最后才是强调色。不同 Agent 撞色等于没有颜色。
pub(crate) fn agent_palette(t: &Theme) -> [crate::color::Rgb; 3] {
    const CHAINS: [&[&str]; 3] = [&["cyan", "blue", "green"], &["orange", "yellow", "green"], &["purple", "blue", "cyan"]];
    let mut used: Vec<crate::color::Rgb> = Vec::new();
    let fallback = t
        .text_action(Variant::Primary, &[State::Selected])
        .rgb()
        .unwrap_or(crate::color::Rgb::opaque(0x56, 0xb6, 0xc2));
    let mut out = [fallback; 3];
    for (slot, chain) in CHAINS.iter().enumerate() {
        let candidate = chain
            .iter()
            .filter_map(|hue| t.hue(hue, 200))
            .chain(t.categorical.iter().filter_map(|scale| scale.get(1).copied()))
            .find(|rgb| !used.contains(rgb));
        if let Some(rgb) = candidate {
            out[slot] = rgb;
            used.push(rgb);
        }
    }
    out
}

/// 当前 Agent 的颜色。Plan 模式按 Plan 的颜色算 —— 在 OpenCode 里 Plan 本来就是一个 Agent。
pub(crate) fn agent_rgb(app: &App, t: &Theme) -> crate::color::Rgb {
    let [echo, build, plan] = agent_palette(t);
    if app.mode == arc_deck_core::mode::Mode::Plan {
        return plan;
    }
    match app.agent.as_str() {
        "build" => build,
        "plan" => plan,
        _ => echo,
    }
}

pub(crate) fn agent_color(app: &App, t: &Theme) -> Color {
    agent_rgb(app, t).to_ratatui()
}

/// 输入框的底色：OpenCode 的 `promptBg = theme.decrease(background.raised.base)`。
/// 抬高面认不出色阶时（自定义主题写死了色值）退到 `background.raised.high`，总之要和页面分得开 ——
/// 没有这块底色，输入框就没有视觉焦点（用户 2026-10-20）。
pub(crate) fn prompt_bg(t: &Theme) -> Color {
    let shifted = t.shift_hue(t.background_raised, -1);
    if shifted != t.background_raised && shifted != t.background {
        shifted.over(t.background)
    } else {
        panel_highlight(t)
    }
}

/// 页脚左边那条「还在干活」的扫描条：OpenCode `ui/spinner.ts` 的 knight-rider（`blocks` 款，
/// 宽 8、两头各停 30 / 9 帧、拖尾 6 级、未亮的格子 0.6 透明度并随停留淡到 0.3），一帧 40 ms。
/// 颜色是当前 Agent 的颜色，所以看一眼就知道是谁在跑。
fn scanner(frame: usize, color: crate::color::Rgb, base: Color) -> Vec<Span<'static>> {
    const WIDTH: usize = 8;
    const HOLD_END: usize = 9;
    const HOLD_START: usize = 30;
    const TRAIL: usize = 6;
    const MIN_ALPHA: f64 = 0.3;
    let total = WIDTH + HOLD_END + (WIDTH - 1) + HOLD_START;
    let f = frame % total;
    // (亮头位置, 停留中, 停留进度, 停留总帧, 移动进度, 移动总帧, 向前)
    let (active, holding, hold, hold_total, moved, move_total, forward) = if f < WIDTH {
        (f, false, 0, 0, f, WIDTH, true)
    } else if f < WIDTH + HOLD_END {
        (WIDTH - 1, true, f - WIDTH, HOLD_END, 0, 0, true)
    } else if f < WIDTH + HOLD_END + WIDTH - 1 {
        let back = f - WIDTH - HOLD_END;
        (WIDTH - 2 - back, false, 0, 0, back, WIDTH - 1, false)
    } else {
        (0, true, f - WIDTH - HOLD_END - (WIDTH - 1), HOLD_START, 0, 0, false)
    };
    let fade = if holding {
        (1.0 - (hold as f64 / hold_total as f64).min(1.0) * (1.0 - MIN_ALPHA)).max(MIN_ALPHA)
    } else {
        MIN_ALPHA + (moved as f64 / (move_total.max(2) - 1) as f64).min(1.0) * (1.0 - MIN_ALPHA)
    };
    let Color::Rgb(br, bg, bb) = base else {
        return vec![Span::styled("■".repeat(WIDTH), Style::default().fg(color.to_ratatui()))];
    };
    let mix = |alpha: f64, bright: f64| {
        let channel = |c: u8, b: u8| {
            let c = (c as f64 * bright).min(255.0);
            (c * alpha + b as f64 * (1.0 - alpha)).round() as u8
        };
        Color::Rgb(channel(color.r, br), channel(color.g, bg), channel(color.b, bb))
    };
    (0..WIDTH)
        .map(|i| {
            let distance = if forward { active as i64 - i as i64 } else { i as i64 - active as i64 };
            let index = if holding {
                distance + hold as i64
            } else if (0..TRAIL as i64).contains(&distance) {
                distance
            } else {
                -1
            };
            if (0..TRAIL as i64).contains(&index) {
                let (alpha, bright) = match index {
                    0 => (1.0, 1.0),
                    1 => (0.9, 1.15),
                    n => (0.65f64.powi(n as i32 - 1), 1.0),
                };
                Span::styled("■", Style::default().fg(mix(alpha, bright)))
            } else {
                Span::styled("⬝", Style::default().fg(mix(0.6 * fade, 1.0)))
            }
        })
        .collect()
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
    // 页脚只有**一行**（照 OpenCode 的 prompt footer）：左边是「现在怎么了」，右边两三个键位。
    // 之前是两行 —— 一行常驻的通知 + 一行八个键位的说明书，用户说「字太多」（2026-10-20）。
    footer_row(frame, app, Rect::new(area.x + 2, area.bottom() - 1, area.width - 4, 1), t);
    let content = Rect::new(area.x, area.y, area.width, area.height - 1);
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
        dim_backdrop(frame);
        popup(frame, app, dialog, content, t);
    } else if app.autocomplete.is_some() {
        autocomplete_popup(frame, app, composer_area, content, t);
    }
    // toast 画在最上层：它是「刚刚发生了什么」的一句话，弹层也不该把它盖住。
    if !app.toasts.is_empty() {
        let h = app.toasts.len() as u16;
        let w = 64.min(area.width.saturating_sub(4));
        let rect = Rect::new(
            area.right().saturating_sub(w + 2),
            area.bottom().saturating_sub(2 + h),
            w,
            h,
        );
        frame.render_widget(Clear, rect);
        let lines = app
            .toasts
            .iter()
            .map(|toast| {
                Line::styled(
                    clean(&toast.text),
                    Style::default()
                        .fg(if toast.error { danger(t) } else { t.text.to_ratatui() })
                        .bg(panel_highlight(t)),
                )
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().bg(panel_highlight(t))),
            rect,
        );
    }
}

/// 页脚那一行。左：通知 > 「Esc 中断」（运行中）> 目录与分支（Home）；右：上下文占用、
/// 内核状态、两个键位（OpenCode 的 `tab agents  ctrl+p commands`）。放不下时先丢右边。
fn footer_row(frame: &mut Frame, app: &App, area: Rect, t: &Theme) {
    use arc_deck_core::service::BackendState;
    let key = |text: String| Span::styled(text, Style::default().fg(self::text(t)));
    let label = |text: &str| Span::styled(text.to_string(), Style::default().fg(muted(t)));
    if app.leader_active() {
        // Which-key, generated from the same keymap that resolves the chord: the strip cannot
        // advertise a combination that no longer exists after a rebind.
        let mut spans = vec![Span::styled(
            format!("{} ", app.keymap.leader_label()),
            Style::default().fg(accent(t)),
        )];
        for (index, (k, title)) in app.leader_hints().into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(key(k));
            spans.push(label(&format!(" {title}")));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    let home = app.selected.is_none() && !app.in_conversation();
    let mut left: Vec<Span<'static>> = Vec::new();
    // 在干活时扫描条**永远在最前面**：它就是用来一眼看出「还在跑」的，不能被一条通知顶掉。
    if app.working() {
        // OpenCode 的「还在工作」：Agent 颜色的扫描条 + `esc interrupt`。
        left.extend(scanner(app.spinner_frame(), agent_rgb(app, t), t.background.to_ratatui()));
        left.push(Span::raw("  "));
        left.push(key("esc".into()));
        left.push(label(" 中断"));
    }
    if !app.notice.is_empty() {
        let alarming = matches!(app.backend, BackendState::Failed { .. })
            || app.boot.config_error.is_some()
            || app.notice.starts_with("错误");
        if !left.is_empty() {
            left.push(Span::raw("  "));
        }
        left.push(Span::styled(
            clean(&app.notice),
            Style::default().fg(if alarming { warning(t) } else { muted(t) }),
        ));
    } else if home && left.is_empty() {
        let mut place = app.boot.workspace.display().to_string();
        if let Some(git) = &app.git {
            place.push_str(&format!(":{}", git.branch));
        }
        left.push(label(&clean(&place)));
    }
    // 右边从左到右排；放不下时从**最前面**开始丢（键位最后丢 —— 它是找到其余一切的入口）。
    let mut right: Vec<Vec<Span<'static>>> = Vec::new();
    if !home && let Some((used, window)) = app.context_usage() {
        let percent = (used as f64 / window as f64 * 100.0).round() as u64;
        right.push(vec![label(&format!("{} · {percent}%", grouped(used)))]);
    }
    match &app.backend {
        BackendState::Ready { .. } => {}
        BackendState::Starting => right.push(vec![label("内核启动中")]),
        BackendState::Failed { .. } => right.push(vec![Span::styled("内核不可用", Style::default().fg(warning(t)))]),
        _ => right.push(vec![label("内核已停止")]),
    }
    if let Some(k) = app.keymap.display(crate::commands::Action::CycleAgent) {
        right.push(vec![key(first_key(&k).to_lowercase()), label(" agents")]);
    }
    if let Some(k) = app.keymap.display(crate::commands::Action::Palette) {
        right.push(vec![key(first_key(&k).to_lowercase()), label(" 命令")]);
    }
    if home {
        right.push(vec![label(&format!("v{}", env!("CARGO_PKG_VERSION")))]);
    }
    let width = |spans: &[Span]| spans.iter().map(|span| span.content.width()).sum::<usize>();
    // 左边只要求留得下 24 格（再长就截断）：一条长通知不该把键位挤掉 —— 实测启动时的
    // 「授权缺口」说明一整行，曾把 `ctrl+p 命令` 挤没了（console-probe，2026-10-20）。
    let left_width = width(&left).min(24).min(area.width as usize);
    loop {
        let used: usize = right.iter().map(|part| width(part) + 2).sum();
        if right.is_empty() || left_width + used <= area.width as usize {
            break;
        }
        right.remove(0);
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (index, part) in right.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.extend(part);
    }
    let right_line = Line::from(spans);
    let rw = right_line.width() as u16;
    if rw > 0 && rw <= area.width {
        frame.render_widget(
            Paragraph::new(right_line),
            Rect::new(area.right() - rw, area.y, rw, 1),
        );
    }
    let room = area.width.saturating_sub(if rw > 0 { rw + 2 } else { 0 });
    if !left.is_empty() && room > 0 {
        let line = Line::from(fit_spans(left, room as usize));
        frame.render_widget(Paragraph::new(line), Rect::new(area.x, area.y, room, 1));
    }
}

/// 把一串 span 截到 `width` 格，**逐个保留样式**，超出时以 `…` 收尾。
/// 之前是拼成一个字符串再用第一段的样式重画 —— 扫描条的颜色会被一条长通知抹平。
fn fit_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(|span| span.content.width()).sum();
    if total <= width {
        return spans;
    }
    let mut out = Vec::new();
    let mut used = 0;
    for span in spans {
        let w = span.content.width();
        if used + w < width {
            used += w;
            out.push(span);
            continue;
        }
        let cut = fit_row(&span.content, width - used);
        out.push(Span::styled(cut, span.style));
        break;
    }
    out
}

/// `Ctrl+End / Alt+G` 只取第一个说法：页脚放不下一张键位表。
fn first_key(display: &str) -> String {
    display.split(" / ").next().unwrap_or(display).trim().to_string()
}

/// 12345 → `12,345`（OpenCode 的 sidebar 写 `toLocaleString()`）。
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn home(frame: &mut Frame, app: &App, area: Rect, t: &Theme) -> Rect {
    // 照 OpenCode 的 Home：品牌、输入框、一条提示，别的都不放。目录 / 版本 / 内核状态在页脚那一行。
    let width = 75.min(area.width.saturating_sub(4));
    let x = area.x + (area.width - width) / 2;
    let height = 7.min(area.height);
    let y = area.y + area.height.saturating_sub(height + 2) / 2;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Arc ", Style::default().fg(muted(t))),
            Span::styled(
                "Deck",
                Style::default().fg(accent(t)).add_modifier(Modifier::BOLD),
            ),
        ]))
        .alignment(ratatui::layout::Alignment::Center),
        Rect::new(x, y, width, 1),
    );
    let composer_area = Rect::new(x, y + 2, width, height.saturating_sub(2));
    composer(frame, app, composer_area, t);
    if area.height > 14 {
        // 只放一条，且是**真实**的命令（§7.3）。交互会话不可用时这一条换成那件事本身。
        let tip = if app.sessions_usable() {
            vec![
                Span::styled("● ", Style::default().fg(warning(t))),
                Span::styled("提示 ", Style::default().fg(text(t))),
                Span::styled("/sessions 打开以前的会话，F1 看全部键位", Style::default().fg(muted(t))),
            ]
        } else {
            vec![Span::styled(
                "交互会话不可用：每次提交是一次独立请求（/reconnect 重试）",
                Style::default().fg(warning(t)),
            )]
        };
        frame.render_widget(
            Paragraph::new(Line::from(tip)),
            Rect::new(x, composer_area.bottom() + 1, width, 1),
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
    // 没有标题栏：OpenCode 的会话页也没有（标题在信息栏里）。省下的那一行给正文。
    // 窄终端看不到信息栏时，标题由 `run` 写进终端窗口标题。
    let width = main.width.saturating_sub(4).max(2);
    let rows = app.editor.wrapped(width.saturating_sub(3) as usize).0.len() as u16;
    let composer_height = (rows + 4).clamp(5, 10).min(main.height.saturating_sub(3));
    // 正文顶上空一行、底下留一行给「回到最新」—— 与 OpenCode 的 marginTop / jump-to-latest 同位。
    let body = Rect::new(
        main.x + 2,
        main.y + 1,
        width,
        main.height.saturating_sub(composer_height + 2),
    );
    // 正文缓存：只在这三件事变了时重排全文（内容、宽度、展开态）。
    let expanded = app
        .expanded
        .get(&format!("session:{}", app.session_id.clone().unwrap_or_default()))
        .copied()
        .unwrap_or(false);
    let key = (
        app.render_revision,
        body.width,
        expanded,
        app.session_id.clone(),
        // 缓存里的 `Line` 带着**样式**：换主题 / 换深浅色必须重排，否则滚动一下
        // 就能看到旧配色的几行和新配色混在一起。
        app.theme.name.clone(),
        app.preferences.light,
    );
    if app.render_cache.as_ref().is_none_or(|cache| cache.key != key) {
        let (lines, offsets) = if app.in_conversation() {
            // `conversation_lines` 自己按宽度折行，偏移量才和最终行号一致。
            conversation_lines(app, t, body.width)
        } else {
            (fit_width(body_lines(app, t, body.width), body.width as usize), Vec::new())
        };
        app.render_cache = Some(crate::app::RenderCache { key, lines, offsets });
        app.render_builds = app.render_builds.wrapping_add(1);
    }
    let (total, offsets) = {
        let cache = app.render_cache.as_ref().expect("上面刚填过");
        (cache.lines.len(), cache.offsets.clone())
    };
    // 时间线跳转要知道每条用户消息落在第几行（w3-session-list 的「跳到某条消息」）。
    app.message_offsets = offsets;
    app.scroll_max = total.saturating_sub(body.height as usize);
    let top = app.scroll.unwrap_or(app.scroll_max).min(app.scroll_max);
    // 只克隆可见的那几行（任务书 w5-polish 第 7 条：渲染只处理可见区域）。
    let visible: Vec<Line<'static>> = app
        .render_cache
        .as_ref()
        .expect("上面刚填过")
        .lines
        .iter()
        .skip(top)
        .take(body.height as usize)
        .cloned()
        .collect();
    // Explicit wrapped rows avoid estimating Ratatui's line count: an estimate caused
    // CJK rows to move the user's scroll anchor whenever progress was appended.
    frame.render_widget(Paragraph::new(visible), body);
    if top < app.scroll_max {
        let key = app.keymap.display(crate::commands::Action::Latest).unwrap_or_default();
        let label = clean(&format!("回到最新 ↓ {}", first_key(&key)));
        let w = (label.width() as u16).min(body.width);
        frame.render_widget(
            Paragraph::new(label).style(Style::default().fg(muted(t))),
            Rect::new(body.right() - w, body.bottom(), w, 1),
        );
    }
    let composer_area = Rect::new(
        main.x + 2,
        main.bottom() - composer_height,
        main.width.saturating_sub(4),
        composer_height,
    );
    composer(frame, app, composer_area, t);
    composer_area
}

fn composer(frame: &mut Frame, app: &App, area: Rect, t: &Theme) {
    if area.height < 3 || area.width < 4 {
        return;
    }
    // 照 OpenCode 的 prompt：左边一条 Agent 颜色的粗竖线 `┃`，里面铺 `promptBg`，最后一行是
    // `╹` + 一排 `▀`（底色的半格，看起来像这块面板的下沿）。之前用的是 `background.formfield`
    // —— OpenCode 自己的主题里它是 `transparent`，于是输入框和页面同色、没有视觉焦点
    // （用户 2026-10-20）。OpenCode 的输入框底色本来就不是 formfield 那个 token，是 promptBg。
    let bg = prompt_bg(t);
    let edge = agent_color(app, t);
    let page = t.background.to_ratatui();
    {
        let buffer = frame.buffer_mut();
        let last = area.bottom() - 1;
        for y in area.top()..area.bottom() {
            let bar = &mut buffer[(area.x, y)];
            bar.set_symbol(if y == last { "╹" } else { "┃" });
            bar.set_fg(edge);
            bar.set_bg(page);
            for x in area.x + 1..area.right() {
                let cell = &mut buffer[(x, y)];
                if y == last {
                    cell.set_symbol("▀");
                    cell.set_fg(bg);
                    cell.set_bg(page);
                } else {
                    cell.set_symbol(" ");
                    cell.set_bg(bg);
                }
            }
        }
    }
    // 行：上留白 · 正文 n 行 · 空一行 · 元数据 · 下留白（OpenCode prompt 的 paddingTop + metadata 行）。
    let inner = Rect::new(area.x + 3, area.y + 1, area.width.saturating_sub(5), area.height.saturating_sub(4).max(1));
    let (rows, (cx, cy)) = app.editor.wrapped(inner.width as usize);
    let start = cy.saturating_sub(inner.height.saturating_sub(1) as usize);
    let visible = if app.editor.text.is_empty() {
        // 窄终端上从后往前丢分句（「…，Shift+Enter 换行」先走），不在词中间截断。
        let mut placeholder = composer_placeholder(app).to_string();
        while placeholder.width() > inner.width as usize {
            match placeholder.rfind('，') {
                Some(cut) => placeholder.truncate(cut),
                None => {
                    placeholder = fit_row(&placeholder, inner.width as usize);
                    break;
                }
            }
        }
        vec![placeholder]
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
        Paragraph::new(visible.join("\n")).style(
            Style::default()
                .bg(bg)
                .fg(if app.editor.text.is_empty() { muted(t) } else { t.text_formfield(&[]).over(t.background) }),
        ),
        inner,
    );
    // 元数据照 OpenCode 的 `PromptMetadataRow`：`Agent · 模型 provider · effort`，Agent 用强调色、
    // 模型用正文色、其余退到次要色。授权级别只在**不是缺省**时出现 —— 缺省值天天看等于噪声，
    // 偏离缺省（readonly / full / confirm）才是该被看见的事。
    //
    // 按**省略顺序**丢，而不是按宽度阈值：模型优先级最高（它决定这一轮到底发给谁）。固定阈值会在
    // 某个宽度上同时跳掉两项，看起来像显示坏了。
    let mut parts: Vec<(u8, Vec<Span<'static>>)> = vec![
        (1, vec![Span::styled(agent_label(app), Style::default().fg(edge))]),
        (0, vec![Span::styled(clean(&app.model_name()), Style::default().fg(text(t)))]),
    ];
    if let Some(model) = &app.model {
        parts.push((4, vec![Span::styled(clean(&model.provider_id), Style::default().fg(muted(t)))]));
    }
    if let Some(effort) = &app.effort {
        parts.push((
            3,
            vec![Span::styled(clean(effort), Style::default().fg(warning(t)).add_modifier(Modifier::BOLD))],
        ));
    }
    if app.permission != "workspace" {
        parts.push((2, vec![Span::styled(clean(&app.permission), Style::default().fg(warning(t)))]));
    }
    let join = |parts: &[(u8, Vec<Span<'static>>)]| {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (index, (priority, part)) in parts.iter().enumerate() {
            // 模型与 provider 之间是空格（「DeepSeek V4.1 Flash opencode-go」），其余用 ` · `。
            if index > 0 {
                let sep = if *priority == 4 { " " } else { " · " };
                spans.push(Span::styled(sep, Style::default().fg(muted(t))));
            }
            spans.extend(part.iter().cloned());
        }
        Line::from(spans)
    };
    while join(&parts).width() > inner.width as usize && parts.len() > 1 {
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
    if area.height >= 5 {
        frame.render_widget(
            Paragraph::new(join(&parts)).style(Style::default().bg(bg)),
            Rect::new(inner.x, area.bottom() - 2, inner.width, 1),
        );
    }
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
    // 没有边线：OpenCode 的 sidebar 只靠抬高的底色和正文分开。
    frame.render_widget(Block::new().style(Style::default().bg(panel_deep(t))), area);
    let inner = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(3),
        area.height.saturating_sub(2),
    );
    let mut lines = Vec::new();
    if app.in_conversation() {
        lines.extend(conversation_sidebar(app, t));
        // 底部：目录、分支、版本（OpenCode 的 sidebar footer）。正文太长时它让位，不和内容抢。
        let mut footer = vec![Line::styled(
            clean(&app.boot.workspace.display().to_string()),
            Style::default().fg(muted(t)),
        )];
        if let Some(git) = &app.git {
            let dirty = if git.changed > 0 { format!(" · {} 个改动", git.changed) } else { String::new() };
            footer.push(Line::styled(clean(&format!("{}{dirty}", git.branch)), Style::default().fg(muted(t))));
        }
        footer.push(Line::from(vec![
            Span::styled("● ", Style::default().fg(ok(t))),
            Span::styled("Saiku ", Style::default().fg(text(t)).add_modifier(Modifier::BOLD)),
            Span::styled(format!("v{}", env!("CARGO_PKG_VERSION")), Style::default().fg(muted(t))),
        ]));
        let footer = wrap(footer, inner.width as usize);
        let content = wrap(lines, inner.width as usize);
        let room = (inner.height as usize).saturating_sub(footer.len() + 1);
        frame.render_widget(
            Paragraph::new(content.into_iter().take(room).collect::<Vec<_>>()),
            inner,
        );
        let h = (footer.len() as u16).min(inner.height);
        frame.render_widget(
            Paragraph::new(footer),
            Rect::new(inner.x, inner.bottom() - h, inner.width, h),
        );
        return;
    }
    lines.push(Line::styled("工作区", Style::default().fg(accent(t))));
    lines.push(Line::raw(clean(&app.boot.workspace.display().to_string())));
    lines.push(Line::raw(""));
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
            "Saiku 采集的事实",
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
/// `final_text` is the model's own statement; `result.summary` is Saiku's fallback
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
    let code_block = ink("code_block");
    // 代码着色用的 `syntax.*`：OpenCode 主题自带这一组，v1 主题迁移后也可能缺几项，
    // 缺的回落到正文 / 状态色，不编一个新的调色板。
    let syntax_ink = |name: &str, fallback: Color| {
        t.syntax(name).map(|ink| ink.over(t.background)).unwrap_or(fallback)
    };
    MarkdownPalette {
        text: ink("text"),
        muted: muted(t),
        heading: ink("heading"),
        link: ink("link"),
        link_text: ink("link_text"),
        code: ink("code"),
        code_block_fg: code_block,
        code_block_bg: panel_highlight(t),
        syntax: crate::syntax::SyntaxPalette {
            plain: code_block,
            comment: syntax_ink("comment", muted(t)),
            keyword: syntax_ink("keyword", accent(t)),
            function: syntax_ink("function", accent(t)),
            r#type: syntax_ink("type", text),
            string: syntax_ink("string", ok(t)),
            number: syntax_ink("number", warning(t)),
            variable: syntax_ink("variable", text),
            operator: syntax_ink("operator", muted(t)),
            punctuation: syntax_ink("punctuation", text),
        },
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
    // 这五个各有自己的内容形状，不走下面那张「名字 + 启用与否」的通用行模型。
    match dialog {
        Dialog::Approval(request) => {
            approval_popup(frame, app, request, area, t);
            return;
        }
        Dialog::Confirm(kind) => {
            confirm_popup(frame, *kind, area, t);
            return;
        }
        Dialog::Search => {
            search_popup(frame, app, area, t);
            return;
        }
        Dialog::Timeline => {
            timeline_popup(frame, app, area, t);
            return;
        }
        Dialog::Viewer(viewer) => {
            viewer_popup(frame, viewer, area, t);
            return;
        }
        _ => {}
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
        Dialog::Sessions | Dialog::Runs | Dialog::Agents | Dialog::Permissions | Dialog::Theme
        | Dialog::Subagents
        | Dialog::Stash
        | Dialog::Approval(_)
        | Dialog::Confirm(_)
        | Dialog::Search
        | Dialog::Timeline
        | Dialog::Viewer(_) => {}
    }
    let rect = dialog_rect(area);
    let title = match dialog {
        Dialog::Sessions => "会话（当前工作区）",
        Dialog::Runs => "工人运行（当前工作区）",
        Dialog::Agents => "Agent",
        Dialog::Permissions => "授权级别",
        Dialog::Subagents => "子 agent 会话",
        Dialog::Stash => "暂存的草稿",
        Dialog::Theme => return,
        // 上面已经把其余分支返回掉了；这里给编译器一个穷尽的分支。
        Dialog::Commands
        | Dialog::Help
        | Dialog::Approval(_)
        | Dialog::Confirm(_)
        | Dialog::Search
        | Dialog::Timeline
        | Dialog::Viewer(_)
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
        Dialog::Subagents => app
            .subagent_rows()
            .into_iter()
            .map(|row| {
                let running = if row.running { " · 进行中" } else { "" };
                (format!("{}{running}", row.label), true)
            })
            .collect(),
        Dialog::Stash => app
            .preferences
            .stash
            .iter()
            .map(|draft| (first_row(&draft.replace('\n', " ⏎ "), 80), true))
            .collect(),
        Dialog::Agents => [
            ("echo", "Echo · 默认工程角色"),
            ("build", "Build · 实施"),
            ("plan", "Plan · 只读规划（执行层拒绝写入与命令）"),
        ]
        .iter()
        .map(|(id, title)| {
            let current = if *id == app.agent.as_str() { "（当前）" } else { "" };
            (format!("{title}{current}"), true)
        })
        .collect(),
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
        let empty = match dialog {
            Dialog::Subagents => "这个会话还没有派出子 agent",
            _ => "没有匹配项",
        };
        rows.push((empty.into(), false));
    }
    if matches!(dialog, Dialog::Runs | Dialog::Sessions) {
        frame.render_widget(
            Paragraph::new(search_line(&app.filter, t)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
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
/// 审批弹层。**不能被折叠、不能被流式正文冲走**（任务书 w4-tui-modes 第 3 条）：
/// 它自己占一块弹层，直到有人回答或收起。
fn approval_popup(frame: &mut Frame, app: &App, request: &ApprovalRequest, area: Rect, t: &Theme) {
    let rect = dialog_rect_tall(area);
    let inner = dialog_frame(frame, rect, "审批：内核在等你回答", t);
    let mut lines = vec![
        Line::styled(
            format!("工具：{}", request.tool_name),
            Style::default().fg(t.text.to_ratatui()).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(call) = &request.call_id {
        lines.push(Line::styled(format!("调用：{call}"), Style::default().fg(muted(t))));
    }
    if let Some(agent) = &request.agent {
        lines.push(Line::styled(format!("Agent：{agent}"), Style::default().fg(muted(t))));
    }
    lines.push(Line::raw(""));
    // 内核给的理由优先用本地化的那条；都没有就说清「没有理由」而不是留白。
    let reason = request
        .display_reason
        .clone()
        .or_else(|| request.reason.clone())
        .unwrap_or_else(|| "内核没有给出理由".into());
    // 工具参数里能读出命令 / 路径的话一并显示：审批时最要紧的是「它要做什么」。
    let detail = app
        .conversation
        .as_ref()
        .and_then(|conversation| {
            toolview::tool_calls(conversation)
                .into_iter()
                .find(|call| Some(&call.call_id) == request.call_id.as_ref())
                .cloned()
        })
        .and_then(|call| call.argument("command").or_else(|| call.argument("path")));
    if let Some(detail) = detail {
        lines.push(Line::styled(clean(&first_row(&detail, 200)), Style::default().fg(warning(t))));
    }
    lines.extend(wrap(
        vec![Line::styled(clean(&reason), Style::default().fg(warning(t)))],
        inner.width as usize,
    ));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "允许这一次只放行这一条；拒绝会让模型收到一次明确的「不」。Esc 只是收起，不是允许。",
        Style::default().fg(muted(t)),
    ));
    if app.approvals.len() > 1 {
        lines.push(Line::styled(
            format!("还有 {} 条待答", app.approvals.len() - 1),
            Style::default().fg(muted(t)),
        ));
    }
    lines.push(Line::styled(
        "y / Enter 允许一次 · n 拒绝 · Esc 稍后回答",
        Style::default().fg(accent(t)),
    ));
    frame.render_widget(
        Paragraph::new(lines.into_iter().take(inner.height as usize).collect::<Vec<_>>()),
        inner,
    );
}

/// 运行中改模式 / Agent 的确认。`y` 才会真的重启内核并打断这一轮。
fn confirm_popup(frame: &mut Frame, kind: ConfirmKind, area: Rect, t: &Theme) {
    let rect = dialog_rect(area);
    let inner = dialog_frame(frame, rect, "确认", t);
    let question = match kind {
        ConfirmKind::Mode(mode) => format!(
            "这一轮还在进行。切到 {} 模式会重启交互内核，当前这一轮会被取消；继续？",
            mode.as_str()
        ),
        ConfirmKind::Agent(agent) => format!(
            "这一轮还在进行。切换到 {} 会重启交互内核（Plan 是只读），当前这一轮会被取消；继续？",
            agent.as_str()
        ),
    };
    let lines = wrap(
        vec![
            Line::styled(clean(&question), Style::default().fg(warning(t))),
            Line::raw(""),
            Line::styled("Enter / y 继续 · Esc / n 取消", Style::default().fg(accent(t))),
        ],
        inner.width as usize,
    );
    frame.render_widget(Paragraph::new(lines), inner);
}

/// 会话内容检索结果。命中词在片段里用 `\u{0}` 夹着，这里换成醒目的一对标记。
fn search_popup(frame: &mut Frame, app: &App, area: Rect, t: &Theme) {
    let rect = dialog_rect_tall(area);
    let inner = dialog_frame(frame, rect, &format!("检索：{}", app.search_query), t);
    let mut lines = vec![Line::styled(
        format!("{} 条命中", app.search.len()),
        Style::default().fg(muted(t)),
    )];
    if app.search.is_empty() {
        lines.push(Line::styled(
            "没有命中。内容是全文索引里的词匹配（不是子串），换一个词试试",
            Style::default().fg(muted(t)),
        ));
    }
    let title_of = |id: &str| {
        app.sessions
            .iter()
            .find(|session| session.session_id == id)
            .and_then(|session| session.title.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let start = app.choice.saturating_sub(inner.height.saturating_sub(3) as usize);
    for (index, hit) in app.search.iter().enumerate().skip(start).take(inner.height.saturating_sub(3) as usize) {
        let focused = index == app.choice;
        let style = if focused {
            let (fg, bg) = row_focus(t);
            Style::default().fg(fg).bg(bg)
        } else {
            Style::default().fg(t.text.to_ratatui())
        };
        let snippet = hit.snippet.replace('\u{0}', "▸");
        lines.push(Line::styled(
            clean(&format!("{} {}", if focused { "›" } else { " " }, title_of(&hit.session_id))),
            style,
        ));
        lines.push(Line::styled(
            clean(&format!("    {}", first_row(&snippet, 300))),
            Style::default().fg(muted(t)),
        ));
    }
    frame.render_widget(
        Paragraph::new(lines.into_iter().take(inner.height as usize).collect::<Vec<_>>()),
        inner,
    );
}

/// 时间线：本会话的用户消息。选中后滚到那一条（`message_offsets` 由正文渲染填）。
fn timeline_popup(frame: &mut Frame, app: &App, area: Rect, t: &Theme) {
    let rect = dialog_rect_tall(area);
    let inner = dialog_frame(frame, rect, "时间线", t);
    let messages = app.user_messages();
    let mut lines = vec![Line::styled(
        format!("{} 条用户消息", messages.len()),
        Style::default().fg(muted(t)),
    )];
    if messages.is_empty() {
        lines.push(Line::styled("这个会话还没有用户消息", Style::default().fg(muted(t))));
    }
    let start = app.choice.saturating_sub(inner.height.saturating_sub(3) as usize);
    for (index, (turn, text)) in messages.iter().enumerate().skip(start).take(inner.height.saturating_sub(3) as usize) {
        let focused = index == app.choice;
        let style = if focused {
            let (fg, bg) = row_focus(t);
            Style::default().fg(fg).bg(bg)
        } else {
            Style::default().fg(t.text.to_ratatui())
        };
        lines.push(Line::styled(
            clean(&format!("{} 第 {turn} 轮 · {}", if focused { "›" } else { " " }, first_row(text, 200))),
            style,
        ));
    }
    frame.render_widget(
        Paragraph::new(lines.into_iter().take(inner.height as usize).collect::<Vec<_>>()),
        inner,
    );
}

/// 全屏查看器：工具完整输出 / diff。可滚动、可过滤。
fn viewer_popup(frame: &mut Frame, viewer: &Viewer, area: Rect, t: &Theme) {
    let rect = Rect::new(
        area.x + 1,
        area.y,
        area.width.saturating_sub(2).max(4),
        area.height,
    );
    let inner = dialog_frame(frame, rect, &clean(&viewer.title), t);
    let query = viewer.filter.to_lowercase();
    let filtered: Vec<&String> = viewer
        .lines
        .iter()
        .filter(|line| query.is_empty() || line.to_lowercase().contains(&query))
        .collect();
    let mut lines = vec![Line::styled(
        format!(
            "{} 行{} · 输入即过滤",
            filtered.len(),
            if query.is_empty() { String::new() } else { format!("（过滤 `{}`）", viewer.filter) }
        ),
        Style::default().fg(muted(t)),
    )];
    let start = viewer.scroll.min(filtered.len().saturating_sub(1));
    for line in filtered.iter().skip(start).take(inner.height.saturating_sub(2) as usize) {
        let style = if line.starts_with('+') {
            Style::default().fg(ok(t))
        } else if line.starts_with('-') {
            Style::default().fg(danger(t))
        } else if line.starts_with("@@") {
            Style::default().fg(accent(t)).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(muted(t))
        };
        lines.push(Line::styled(clean(line), style));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

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
    dialog_panel(
        frame,
        rect,
        Line::styled(clean(title), Style::default().fg(text(t)).add_modifier(Modifier::BOLD)),
        t,
    )
}

/// 对话框里的搜索行：空着时是次要色的「搜索」占位，输入后是正文色的过滤串（OpenCode 的 `Search`）。
pub(crate) fn search_line(filter: &str, t: &Theme) -> Line<'static> {
    if filter.is_empty() {
        Line::styled("搜索", Style::default().fg(muted(t)))
    } else {
        Line::styled(clean(filter), Style::default().fg(text(t)))
    }
}

/// 所有弹层共用的面板，照 OpenCode 的 `Dialog`：抬高的底色、**没有边框**、粗体标题在左、
/// `esc` 在右，正文从标题下空一行开始。之前是 `┌标题──┐` 的方框，标题挤在边线里，
/// 每个弹层底下还挂一行「↑↓ 选择 · Enter 执行 · Esc 返回」（2026-10-20 用户：字太多）。
/// 背后的页面在 `draw` 里压暗（[`dim_backdrop`]）。
pub(crate) fn dialog_panel(frame: &mut Frame, rect: Rect, title: Line<'static>, t: &Theme) -> Rect {
    frame.render_widget(Clear, rect);
    frame.render_widget(Block::new().style(Style::default().bg(dialog_bg(t))), rect);
    if rect.height >= 3 && rect.width >= 8 {
        let row = Rect::new(rect.x + 2, rect.y + 1, rect.width - 4, 1);
        frame.render_widget(Paragraph::new(title), row);
        frame.render_widget(
            Paragraph::new("esc")
                .style(Style::default().fg(muted(t)))
                .alignment(ratatui::layout::Alignment::Right),
            row,
        );
    }
    Rect::new(
        rect.x + 2,
        rect.y + 3,
        rect.width.saturating_sub(4),
        rect.height.saturating_sub(4),
    )
}

/// 弹层背后的页面压暗：前景与背景都往黑里压（OpenCode 盖一层 `RGBA(0,0,0,150)`）。
/// 只动 RGB 色；`Reset` 这类终端缺省色没法按比例压，交给弹层自己的底色盖住。
fn dim_backdrop(frame: &mut Frame) {
    fn dim(color: Color) -> Color {
        match color {
            Color::Rgb(r, g, b) => {
                let k = |v: u8| ((v as u16 * 105) / 255) as u8;
                Color::Rgb(k(r), k(g), k(b))
            }
            other => other,
        }
    }
    let area = frame.area();
    let buffer = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buffer[(x, y)];
            let (fg, bg) = (cell.fg, cell.bg);
            cell.set_fg(dim(fg));
            cell.set_bg(dim(bg));
        }
    }
}

/// Command palette. Rows are grouped by category and carry the keybinding the keymap really
/// resolves; an action the backend cannot serve stays listed, greyed, and its reason is shown
/// on the footer line when it is the selected row.
fn commands_popup(frame: &mut Frame, app: &App, area: Rect, t: &Theme) {
    let rect = dialog_rect_tall(area);
    let inner = dialog_frame(frame, rect, "命令", t);
    frame.render_widget(
        Paragraph::new(search_line(&app.filter, t)),
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
                clean(title),
                Style::default()
                    .fg(accent(t))
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
    // 页脚只在选中行**用不了**时说为什么；平时空着（↑↓ / Enter 不需要说明书）。
    let footer = selected.and_then(|action| app.availability(action)).unwrap_or_default();
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
    let total = lines.len();
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
    let more = if total > start + visible { "↓ 还有" } else { "" };
    frame.render_widget(
        Paragraph::new(more).style(Style::default().fg(muted(t))),
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
    // 照 OpenCode 的 autocomplete：输入框正上方一块抬高的列表，没有标题、没有边框。
    let rows = popup.items.len().min(8);
    let width = 64.min(area.width.saturating_sub(2)).max(20);
    let height = (rows as u16).min(area.height.saturating_sub(2)).max(1);
    let x = composer
        .x
        .min(area.right().saturating_sub(width))
        .max(area.x);
    let y = composer.y.saturating_sub(height).max(area.y);
    let rect = Rect::new(x, y, width, height);
    frame.render_widget(Clear, rect);
    frame.render_widget(Block::new().style(Style::default().bg(dialog_bg(t))), rect);
    let inner = Rect::new(rect.x + 1, rect.y, rect.width.saturating_sub(2), rect.height);
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
        // 选中行的底色铺满整行，不只到文字末尾。
        let text = clean(&text);
        let pad = (inner.width as usize).saturating_sub(text.width());
        lines.push(Line::styled(format!("{text}{}", " ".repeat(pad)), style));
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

fn composer_placeholder(app: &App) -> &'static str {
    use arc_deck_core::service::BackendState;
    if !matches!(app.backend, BackendState::Ready { .. } | BackendState::Starting) {
        "输入指令，Enter 发送（每次提交是一次独立运行）"
    } else if app.conversation.as_ref().is_some_and(|c| c.running) {
        "输入下一条，这一轮结束后发送"
    } else if app.session_id.is_some() {
        "继续这个会话，Enter 发送"
    } else {
        // 键位提示放在占位符里（OpenCode 的 `Ask anything...` 那个位置）—— 页脚不再挂说明书，
        // 但「怎么发送」必须有地方说（2026-10-20 的 F6 教训）。
        "输入消息，Enter 发送，Shift+Enter 换行"
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
    // 照 OpenCode 的 sidebar：粗体标题，然后一节一节（粗体小标题 + 次要色的值），节与节之间空一行。
    // 模型 / Agent / 授权已经在输入框下面了，这里不再写一遍；「状态 空闲」「N 轮」这类不帮人做
    // 决定的行也去掉（2026-10-20 用户：字太多）。没有数据的节整块省略，不写占位（§7.3）。
    let heading = |name: &str| Line::styled(name.to_string(), Style::default().fg(text(t)).add_modifier(Modifier::BOLD));
    lines.push(Line::styled(
        clean(c.title.as_deref().unwrap_or("新会话")),
        Style::default().fg(text(t)).add_modifier(Modifier::BOLD),
    ));
    let mut total = arc_deck_core::conversation::TokenUsage::default();
    for message in &c.messages {
        if let Message::Assistant(turn) = message
            && let Some(usage) = &turn.usage
        {
            total.add(usage);
        }
    }
    // 只显示 provider 真的报告过的数（§7.3：信息栏只显示真实数据）。
    let context = app.context_usage();
    if context.is_some() || total.input.is_some() || total.output.is_some() {
        lines.push(Line::raw(""));
        lines.push(heading("上下文"));
        match context {
            Some((used, window)) => {
                let percent = (used as f64 / window as f64 * 100.0).round() as u64;
                lines.push(Line::styled(format!("{} tokens", grouped(used)), Style::default().fg(muted(t))));
                lines.push(Line::styled(format!("{percent}% 已用"), Style::default().fg(muted(t))));
            }
            None => lines.push(Line::styled(
                format!("输入 {} · 输出 {}", token(total.input), token(total.output)),
                Style::default().fg(muted(t)),
            )),
        }
    }
    let changed = app.changed_files();
    if !changed.is_empty() {
        lines.push(Line::raw(""));
        lines.push(heading("改动的文件"));
        for path in changed.iter().take(6) {
            lines.push(Line::styled(clean(path), Style::default().fg(muted(t))));
        }
        if changed.len() > 6 {
            lines.push(Line::styled(format!("+{} 个", changed.len() - 6), Style::default().fg(muted(t))));
        }
    }
    let todos = app.task_todos();
    if !todos.is_empty() {
        lines.push(Line::raw(""));
        lines.push(heading("Todo"));
        for (item, done) in todos.iter().take(8) {
            lines.push(Line::styled(
                clean(&format!("[{}] {item}", if *done { "✓" } else { " " })),
                Style::default().fg(if *done { muted(t) } else { text(t) }),
            ));
        }
    }
    if !app.approvals.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("待答审批 · {}", app.approvals.len()),
            Style::default().fg(warning(t)).add_modifier(Modifier::BOLD),
        ));
    }
    if !c.history_complete {
        lines.push(Line::raw(""));
        lines.push(Line::styled("更早的历史没有取全", Style::default().fg(warning(t))));
    }
    lines
}

/// 助手正文、思考、工具行的左缩进 —— OpenCode 的 `paddingLeft={3}`。用户消息不缩进（它有竖线）。
const INDENT: &str = "   ";

/// 会话正文。
///
/// 形状照 OpenCode v1.19.0 的会话页（`routes/session/index.tsx` + `message-parts.tsx`）：
/// - 用户消息：左竖线 + 抬高底色，上下各留一行内边距，不加「你」这类角色标签；
/// - 助手：不加角色标签，正文缩进 3 列；一轮结束后一行 `▣ Agent · 模型`（`AssistantFooter`）；
/// - 工具：一行、次要色、图标 + 名字 + 关键参数，**不写「完成」**（✓ 已经说了）；
///   连续的只读查看（read / glob / grep）折成一行「浏览了 …」（OpenCode 的 exploration 分组）。
///
/// 折叠规则（§7.3）：思考与工具输出默认折叠，`/tools` 展开；**失败与被拒在折叠态也看得见** ——
/// 折叠是为了少占屏幕，不是为了把出错藏起来。
fn conversation_lines(app: &App, t: &Theme, width: u16) -> (Vec<Line<'static>>, Vec<(u32, usize)>) {
    let mut lines = Vec::new();
    let mut offsets = Vec::new();
    let expanded = app
        .expanded
        .get(&format!("session:{}", app.session_id.clone().unwrap_or_default()))
        .copied()
        .unwrap_or(false);
    let user_block = |user: &arc_deck_core::conversation::UserMessage, lines: &mut Vec<Line<'static>>| {
        let inner = width.saturating_sub(3) as usize;
        let bg = panel_deep(t);
        // `┃`：OpenCode 的 SplitBorder；颜色是 Agent 的颜色（`UserMessage` 的 borderColor）。
        let bar = || Span::styled("┃", Style::default().fg(agent_color(app, t)).bg(bg));
        let blank = || Line::from(vec![bar(), Span::styled(" ".repeat(inner + 2), Style::default().bg(bg))]);
        let mut text: Vec<Line> = clean(&user.text).lines().map(|s| Line::raw(s.to_string())).collect();
        if !user.attachments.is_empty() {
            text.push(Line::raw(""));
            for attachment in &user.attachments {
                text.push(Line::from(vec![
                    Span::styled(" file ", Style::default().fg(bg).bg(accent(t)).add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" {} ", clean(attachment)), Style::default().fg(muted(t))),
                ]));
            }
        }
        lines.push(blank());
        for line in wrap(text, inner) {
            let used = line.width().min(inner);
            let mut spans = vec![bar(), Span::styled("  ", Style::default().bg(bg))];
            spans.extend(line.spans.into_iter().map(|span| {
                let style = if span.style.fg.is_none() { span.style.fg(text_on(t)) } else { span.style };
                Span::styled(span.content.into_owned(), style.bg(bg))
            }));
            spans.push(Span::styled(" ".repeat(inner.saturating_sub(used)), Style::default().bg(bg)));
            lines.push(Line::from(spans));
        }
        lines.push(blank());
    };
    let footer = AssistantFooter {
        agent: agent_label(app),
        color: agent_color(app, t),
        // 与输入框同一个说法（目录里的展示名），不是内核的原始 id —— 同一屏两种写法看着像两个模型。
        model: app.conversation.as_ref().and_then(|c| c.model.as_ref()).map(|m| {
            crate::catalog::display_name(
                &app.boot.catalog,
                &crate::catalog::ModelRef { provider_id: m.provider.clone(), model_id: m.model.clone() },
            )
        }),
    };
    if let Some(c) = &app.conversation {
        if !c.history_complete {
            lines.push(Line::styled(
                format!("{INDENT}更早的历史没有取全"),
                Style::default().fg(warning(t)),
            ));
        }
        for message in &c.messages {
            match message {
                Message::User(user) => {
                    if !lines.is_empty() {
                        lines.push(Line::raw(""));
                    }
                    offsets.push((user.turn, lines.len()));
                    user_block(user, &mut lines);
                }
                Message::Assistant(turn) => {
                    let body = assistant_lines(turn, t, width, expanded, &footer);
                    if !body.is_empty() {
                        lines.push(Line::raw(""));
                        lines.extend(body);
                    }
                }
                Message::Unknown => {}
            }
        }
    }
    if let Some(text) = &app.pending {
        let user = arc_deck_core::conversation::UserMessage {
            turn: 0,
            text: text.clone(),
            attachments: Vec::new(),
        };
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        user_block(&user, &mut lines);
        lines.push(Line::raw(""));
        lines.push(Line::styled(format!("{INDENT}⋯ 发送中"), Style::default().fg(muted(t))));
    }
    if lines.is_empty() {
        lines.push(Line::styled(format!("{INDENT}正在打开会话…"), Style::default().fg(muted(t))));
    }
    (lines, offsets)
}

/// 用户消息正文的颜色。抬高的底色上用 `text.base`，与 OpenCode 的 `UserMessage` 一致。
fn text_on(t: &Theme) -> Color {
    t.text.over(t.background_raised)
}

/// 一轮结束后的那一行要的东西。
struct AssistantFooter {
    agent: String,
    color: Color,
    model: Option<String>,
}

/// 输入框与轮次页脚共用的 Agent 说法：`Echo`，Plan 模式下 `Echo · Plan`。
pub(crate) fn agent_label(app: &App) -> String {
    let mut chars = app.agent.chars();
    let name = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    };
    if app.mode == arc_deck_core::mode::Mode::Plan {
        format!("{name} · Plan")
    } else {
        name
    }
}

fn assistant_lines(
    turn: &AssistantTurn,
    t: &Theme,
    width: u16,
    expanded: bool,
    footer: &AssistantFooter,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(INDENT.len() as u16).max(2);
    let mut lines: Vec<Line<'static>> = Vec::new();
    // 段与段之间空一行（OpenCode 每个 row 的 marginTop），**连续的工具之间不空** —— 一串工具是一段。
    // `previous` 记上一段是不是工具；`None` = 还没有任何一段。
    let mut previous: Option<bool> = None;
    let gap = |lines: &mut Vec<Line<'static>>, previous: Option<bool>, tool: bool| {
        if previous.is_some_and(|was_tool| !(was_tool && tool)) {
            lines.push(Line::raw(""));
        }
    };
    let blocks = &turn.blocks;
    let mut index = 0;
    while index < blocks.len() {
        let mut step = 1;
        match &blocks[index] {
            ViewBlock::Text(part) if !part.text.trim().is_empty() || part.streaming => {
                gap(&mut lines, previous, false);
                let mut body = indent(markdown::render(part.text.trim(), inner, &markdown_palette(t)));
                if part.streaming {
                    let cursor = Span::styled("▍", Style::default().fg(accent(t)));
                    match body.last_mut() {
                        Some(last) => last.spans.push(cursor),
                        None => body.push(Line::from(vec![Span::raw(INDENT), cursor])),
                    }
                }
                lines.extend(body);
                previous = Some(false);
            }
            ViewBlock::Reasoning(part) if !part.text.trim().is_empty() || part.streaming => {
                gap(&mut lines, previous, false);
                let (title, body) = reasoning_summary(&part.text);
                let head = match (part.streaming, &title) {
                    (true, Some(title)) => format!("⋯ 思考：{title}"),
                    (true, None) => "⋯ 思考中".into(),
                    (false, Some(title)) => format!("{} 思考：{title}", if expanded { "-" } else { "+" }),
                    (false, None) => format!("{} 思考", if expanded { "-" } else { "+" }),
                };
                lines.push(Line::styled(
                    fit_row(&format!("{INDENT}{head}"), width as usize),
                    Style::default().fg(warning(t)),
                ));
                if expanded {
                    let rows = wrap(
                        clean(&body).lines().map(|s| Line::raw(s.to_string())).collect(),
                        inner.saturating_sub(2) as usize,
                    );
                    for row in rows {
                        let mut spans = vec![Span::styled(format!("{INDENT}│ "), Style::default().fg(t.border.to_ratatui()))];
                        spans.extend(row.spans.into_iter().map(|span| {
                            Span::styled(
                                span.content.into_owned(),
                                Style::default().fg(muted(t)).add_modifier(Modifier::ITALIC),
                            )
                        }));
                        lines.push(Line::from(spans));
                    }
                }
                previous = Some(false);
            }
            ViewBlock::Tool(call) => {
                gap(&mut lines, previous, true);
                // 连续的只读查看折成一行（≥2 个、都成功、没展开）。失败的那个照常单独一行。
                let run: Vec<&ToolCall> = blocks[index..]
                    .iter()
                    .map_while(|block| match block {
                        ViewBlock::Tool(call)
                            if toolview::kind(&call.name) == toolview::Kind::Inspect
                                && call.status == ToolStatus::Done
                                && !tool_failed(call) =>
                        {
                            Some(call)
                        }
                        _ => None,
                    })
                    .collect();
                if !expanded && run.len() >= 2 {
                    lines.push(explored_line(&run, t, width));
                    step = run.len();
                } else {
                    lines.extend(tool_lines(call, t, width, expanded));
                }
                previous = Some(true);
            }
            _ => {}
        }
        index += step;
    }
    let mut tail: Vec<Span<'static>> = Vec::new();
    let mut error: Option<String> = None;
    match &turn.state {
        TurnState::Running => {
            if lines.is_empty() {
                lines.push(Line::styled(format!("{INDENT}⋯ 思考中"), Style::default().fg(warning(t))));
            }
            return lines;
        }
        TurnState::Completed | TurnState::Unknown => {}
        TurnState::Cancelled => tail.push(Span::raw("已取消")),
        TurnState::Failed { message, code } => {
            let code = code.as_deref().map(|c| format!("[{c}] ")).unwrap_or_default();
            error = Some(format!("{code}{message}"));
        }
        TurnState::MaxTokens => tail.push(Span::styled("达到输出上限", Style::default().fg(warning(t)))),
        TurnState::Blocked => tail.push(Span::styled("被阻塞", Style::default().fg(warning(t)))),
        TurnState::Interrupted => tail.push(Span::styled("内核崩溃时中断", Style::default().fg(warning(t)))),
        TurnState::Other { kind } => tail.push(Span::raw(clean(kind))),
    }
    if turn.retries > 0 {
        tail.push(Span::raw(format!("重试 {} 次", turn.retries)));
    }
    if !lines.is_empty() {
        lines.push(Line::raw(""));
    }
    if let Some(error) = error {
        for row in wrap(vec![Line::raw(clean(&format!("错误：{error}")))], inner as usize) {
            let mut spans = vec![Span::raw(INDENT)];
            spans.extend(row.spans.into_iter().map(|span| Span::styled(span.content.into_owned(), Style::default().fg(danger(t)))));
            lines.push(Line::from(spans));
        }
    }
    let mut spans = vec![
        Span::styled(format!("{INDENT}▣ "), Style::default().fg(footer.color)),
        Span::styled(
            clean(&footer.agent),
            Style::default().fg(if turn.state == TurnState::Completed { footer.color } else { muted(t) }),
        ),
    ];
    if let Some(model) = &footer.model {
        spans.push(Span::styled(format!(" · {}", clean(model)), Style::default().fg(muted(t))));
    }
    for part in tail {
        let style = if part.style.fg.is_some() { part.style } else { Style::default().fg(muted(t)) };
        spans.push(Span::styled(" · ", Style::default().fg(muted(t))));
        spans.push(Span::styled(part.content.into_owned(), style));
    }
    lines.push(Line::from(spans));
    lines
}

/// OpenCode 的 `reasoningSummary`：正文以 `**标题**` 开头时取它当标题，其余是正文。
fn reasoning_summary(text: &str) -> (Option<String>, String) {
    let content = text.trim();
    if let Some(rest) = content.strip_prefix("**")
        && let Some(end) = rest.find("**")
    {
        let title = &rest[..end];
        let after = &rest[end + 2..];
        if !title.contains('\n') && !title.trim().is_empty() && (after.is_empty() || after.starts_with('\n')) {
            return (Some(title.trim().to_string()), after.trim().to_string());
        }
    }
    (None, content.to_string())
}

/// 每一行前面加三格缩进。
fn indent(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let mut spans = vec![Span::raw(INDENT)];
            spans.extend(line.spans);
            Line::from(spans).style(line.style)
        })
        .collect()
}

/// 截到一行，超出的用 `…` 收尾（按终端格宽，不按字节 —— 中文一个字两格）。
fn fit_row(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for g in text.graphemes(true) {
        let n = g.width();
        if used + n + 1 > width {
            break;
        }
        out.push_str(g);
        used += n;
    }
    out.push('…');
    out
}

fn tool_failed(call: &ToolCall) -> bool {
    matches!(call.status, ToolStatus::Failed) || call.exit_code.is_some_and(|code| code != 0)
}

/// 工具名的显示形式：首字母大写（OpenCode 写 `Read` / `Edit`，DSH 的名字是小写的）。
fn tool_title(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// 连续几次只读查看的合并行，例如 `→ 浏览了 3 个文件、2 次搜索`。
fn explored_line(run: &[&ToolCall], t: &Theme, width: u16) -> Line<'static> {
    let reads = run.iter().filter(|call| matches!(call.name.as_str(), "read")).count();
    let searches = run.len() - reads;
    let mut parts = Vec::new();
    if reads > 0 {
        parts.push(format!("{reads} 个文件"));
    }
    if searches > 0 {
        parts.push(format!("{searches} 次查找"));
    }
    Line::styled(
        fit_row(&format!("{INDENT}→ 浏览了 {}", parts.join("、")), width as usize),
        Style::default().fg(muted(t)),
    )
}

/// 一次工具调用：一行摘要 + 按需的输出。
///
/// 照 OpenCode 的 `InlineTool`：整行次要色，失败才变红；不写「完成」「进行中」这类状态词 ——
/// 图标已经说了（✓ / ⋯ / ✗）。编辑类工具把 `+N −M` 放在同一行，展开时画着色 diff
/// （任务书 w3-tool-views）。之前的版本每个工具两行、行尾还挂着「（/tools 展开 diff，/diff 看全部）」，
/// 一轮十几个工具就是一屏说明书（2026-10-20 用户：字太多）。
fn tool_lines(call: &ToolCall, t: &Theme, width: u16, expanded: bool) -> Vec<Line<'static>> {
    let failed = tool_failed(call);
    let color = if failed {
        danger(t)
    } else if call.status == ToolStatus::Abandoned {
        warning(t)
    } else {
        muted(t)
    };
    let icon = toolview::mark(call);
    let mut head = format!("{INDENT}{icon} {}{}", tool_title(&call.name), clean(&toolview::subject(call)));
    let counts = toolview::counts(call);
    if !counts.is_empty() {
        head.push_str(&counts.replacen(" · ", "  ", 1));
    }
    if let Some(code) = call.exit_code.filter(|code| *code != 0) {
        head.push_str(&format!(" · exit {code}"));
    }
    if call.status == ToolStatus::Abandoned {
        head.push_str(" · 未完成");
    }
    let mut lines = vec![Line::styled(fit_row(&head, width as usize), Style::default().fg(color))];
    let body_indent = format!("{INDENT}  ");
    let room = (width as usize).saturating_sub(body_indent.width());
    let push_row = |lines: &mut Vec<Line<'static>>, row: &str, style: Style| {
        lines.push(Line::styled(format!("{body_indent}{}", fit_row(&clean(row), room)), style));
    };
    if let Some(error) = &call.error {
        for row in error.lines().filter(|row| !row.trim().is_empty()).take(3) {
            push_row(&mut lines, row, Style::default().fg(danger(t)));
        }
    }
    match toolview::kind(&call.name) {
        toolview::Kind::Edit if expanded => {
            let diffs = toolview::diffs_of(call);
            let mut budget = 160usize;
            'outer: for diff in &diffs {
                for line in toolview::diff_lines(diff) {
                    if budget == 0 {
                        push_row(&mut lines, "…（/diff 看全部）", Style::default().fg(muted(t)));
                        break 'outer;
                    }
                    let style = match line.kind {
                        toolview::DiffLineKind::Meta => Style::default().fg(accent(t)),
                        toolview::DiffLineKind::Added => Style::default().fg(ok(t)),
                        toolview::DiffLineKind::Removed => Style::default().fg(danger(t)),
                        toolview::DiffLineKind::Context => Style::default().fg(muted(t)),
                    };
                    push_row(&mut lines, &line.text, style);
                    budget -= 1;
                }
            }
            if diffs.is_empty()
                && let Some(output) = &call.output
            {
                for row in output.lines().filter(|line| !line.trim().is_empty()).take(40) {
                    push_row(&mut lines, row, Style::default().fg(muted(t)));
                }
            }
        }
        toolview::Kind::Edit if !failed => {}
        _ => {
            if let Some(output) = &call.output {
                let rows: Vec<&str> = output.lines().filter(|line| !line.trim().is_empty()).collect();
                let limit = if expanded { 40 } else { toolview::folded_lines(call) };
                // 失败时从**开头**取（拒因 / 栈顶通常在前面），成功时取尾部（命令输出的重点在结尾）。
                let shown: Vec<&&str> = if failed {
                    rows.iter().take(limit).collect()
                } else {
                    rows.iter().rev().take(limit).rev().collect()
                };
                let style = Style::default().fg(muted(t));
                for row in &shown {
                    push_row(&mut lines, row, style);
                }
                if limit > 0 && rows.len() > limit {
                    push_row(&mut lines, &format!("… +{} 行", rows.len() - limit), style);
                }
            }
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
    use crate::{Boot, Preferences, RunDetail, RunRecord, Update};
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
                personalization: false,
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
    fn failure_details_are_visible_even_when_tools_are_folded() {        let mut a = app();
        long_run(&mut a);
        let text = body_lines(&a, &a.theme.clone(), 80)
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("exit 101"));
        assert_eq!(text.matches("第0行中文结果").count(), 1);
        assert!(text.contains("Saiku 采集的事实"));
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
        // 输入框正文从 x+3 起（竖线 + 两格内边距）：5 + 「中文é」的 5 格。
        assert_eq!(cursor.x, 10);
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
        // 列表会滚（照 OpenCode）；要一屏看全四个分类得有足够的行 —— 40 行在加了 /stash、
        // /subagents 两条之后放不下最后一类，那是「在下面」不是「没分类」。
        let mut terminal = Terminal::new(TestBackend::new(120, 50)).unwrap();
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
        assert!(contains_wrapped(buffer, "Ctrl+X"));
        assert!(contains_wrapped(buffer, "新会话"));
    }

    /// 元数据的省略顺序：模型最高，然后 Agent、授权级别。窄终端上必须仍看得见模型，
    /// 而不是几个字段一起消失或按固定阈值一下跳掉两项。授权级别只在偏离缺省时出现，
    /// 所以这里用 `full`。
    #[test]
    fn metadata_drops_low_priority_parts_as_the_terminal_narrows() {
        let mut a = app();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        assert!(!contains_wrapped(terminal.backend().buffer(), "workspace"), "缺省授权不占地方");
        for (width, height, workspace, agent) in [
            (120, 30, true, true),
            (40, 12, false, true),
            (30, 12, false, false),
        ] {
            let mut a = app();
            a.permission = "full".into();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| draw(f, &mut a)).unwrap();
            let buffer = terminal.backend().buffer();
            assert!(
                contains_wrapped(buffer, "DeepSeek V4.1 Flash"),
                "the model name is never dropped ({width}x{height})"
            );
            assert_eq!(
                contains_wrapped(buffer, "· full"),
                workspace,
                "permission goes before the agent ({width}x{height})"
            );
            assert_eq!(
                contains_wrapped(buffer, "Echo"),
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
                for screen in ["home", "run", "commands", "theme", "help", "permissions", "models", "conversation"] {
                    let mut a = if screen == "conversation" { agentic_app() } else { app() };
                    set_mode(&mut a, light);
                    if !matches!(screen, "home" | "conversation" | "models") {
                        long_run(&mut a);
                    }
                    if screen == "models" {
                        a.open_dialog(Dialog::Models(crate::model_dialog::ModelDialogState::default()));
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

    // ── M4 w5-polish / w3-tool-views：信息栏、缓存、diff、审批 ────────────────

    fn conversation_app() -> App {
        use arc_deck_core::conversation::{AssistantTurn, Block, ToolCall, ToolStatus, TurnState};
        let mut a = app();
        a.session_id = Some("s1".into());
        let mut edit = ToolCall {
            call_id: "c1".into(),
            name: "edit".into(),
            arguments: r#"{"file_path":"src/lib.rs"}"#.into(),
            status: ToolStatus::Done,
            output: Some("ok".into()),
            ..ToolCall::default()
        };
        edit.detail = Some(serde_json::json!({
            "diffs": [{ "path": "src/lib.rs", "oldText": "let a = 1;\nlet b = 2;", "newText": "let a = 1;\nlet b = 3;" }]
        }));
        let turn = AssistantTurn {
            turn: 1,
            blocks: vec![Block::Tool(edit)],
            state: TurnState::Completed,
            ..AssistantTurn::default()
        };
        a.update(Update::Session(arc_deck_core::service::Event::Conversation {
            conversation: Box::new(arc_deck_core::conversation::Conversation {
                session_id: "s1".into(),
                title: Some("测试会话".into()),
                cwd: Some("C:/项目/arc-deck".into()),
                messages: vec![
                    Message::User(arc_deck_core::conversation::UserMessage {
                        turn: 1,
                        text: "改一下".into(),
                        attachments: Vec::new(),
                    }),
                    Message::Assistant(turn),
                ],
                history_complete: true,
                running: false,
                model: None,
            }),
        }));
        a.git = Some(crate::app::GitStatus { branch: "main".into(), changed: 2 });
        a
    }

    /// 一轮像样的 agentic 回合：思考、几次查看、一次编辑、一次失败的命令、结论。
    /// 密度调整（2026-10-20）靠它出预览，也靠它断言「每个工具一行」。
    fn agentic_app() -> App {
        use arc_deck_core::conversation::{AssistantTurn, Block, ModelChoice, TextPart, ToolCall, ToolStatus, TurnState};
        let mut a = app();
        a.session_id = Some("s1".into());
        let tool = |name: &str, args: &str, output: &str| ToolCall {
            call_id: name.into(),
            name: name.into(),
            arguments: args.into(),
            status: ToolStatus::Done,
            output: Some(output.into()),
            ..ToolCall::default()
        };
        let mut edit = tool("edit", r#"{"file_path":"src/lib.rs"}"#, "ok");
        edit.detail = Some(serde_json::json!({
            "diffs": [{ "path": "src/lib.rs", "oldText": "let a = 1;\nlet b = 2;", "newText": "let a = 1;\nlet b = 3;" }]
        }));
        let mut test = tool("bash", r#"{"command":"cargo test -p demo"}"#, "running 3 tests\ntest a ... ok\ntest b ... FAILED\nfailures:\n  b: assertion failed\n  left: 2\n  right: 3");
        test.exit_code = Some(101);
        let text = |s: &str| Block::Text(TextPart { text: s.into(), ..TextPart::default() });
        let turn = AssistantTurn {
            turn: 1,
            blocks: vec![
                Block::Reasoning(TextPart { text: "**定位 b 的取值**\n\n先看 lib.rs 再跑测试。".into(), ..TextPart::default() }),
                text("我先看一下相关代码。"),
                Block::Tool(tool("read", r#"{"path":"src/lib.rs"}"#, "1 let a = 1;")),
                Block::Tool(tool("grep", r#"{"pattern":"let b"}"#, "src/lib.rs:2")),
                Block::Tool(tool("read", r#"{"path":"tests/b.rs"}"#, "…")),
                Block::Tool(edit),
                Block::Tool(test),
                text("把 `b` 改成了 `3`，但 **测试 b 仍然失败**：它断言的是旧值。\n\n- 下一步：改测试的期望值\n- 或者回退这次修改"),
            ],
            state: TurnState::Completed,
            ..AssistantTurn::default()
        };
        a.update(Update::Session(arc_deck_core::service::Event::Conversation {
            conversation: Box::new(arc_deck_core::conversation::Conversation {
                session_id: "s1".into(),
                title: Some("修 b 的取值".into()),
                cwd: Some("C:/项目/arc-deck".into()),
                messages: vec![
                    Message::User(arc_deck_core::conversation::UserMessage {
                        turn: 1,
                        text: "把 b 改成 3，然后跑一下测试".into(),
                        attachments: Vec::new(),
                    }),
                    Message::Assistant(turn),
                ],
                history_complete: true,
                running: false,
                model: Some(ModelChoice {
                    provider: "opencode-go".into(),
                    model: "deepseek-v4.1-flash".into(),
                    effort: None,
                }),
            }),
        }));
        a.git = Some(crate::app::GitStatus { branch: "main".into(), changed: 2 });
        a
    }

    #[test]
    fn an_agentic_turn_is_one_row_per_tool() {
        let mut a = agentic_app();
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let text = row_dump(terminal.backend().buffer());
        assert!(text.contains("→ 浏览了 2 个文件、1 次查找"), "连续的查看折成一行：{text}");
        assert!(text.contains("← Edit src/lib.rs  +1 −1"), "{text}");
        assert!(text.contains("✗ Bash cargo test -p demo · exit 101"), "{text}");
        assert!(text.contains("+ 思考：定位 b 的取值"), "{text}");
        assert!(text.contains("▣ Echo · DeepSeek V4.1 Flash"), "与输入框同一个展示名：{text}");
        assert!(!text.contains("完成"), "✓ 已经说了，不写状态词：{text}");
        assert!(!text.contains("/tools 展开"), "工具行不挂说明：{text}");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tui-preview");
        std::fs::create_dir_all(&dir).unwrap();
        for (w, h) in [(80, 30), (100, 40), (160, 40)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|f| draw(f, &mut a)).unwrap();
            std::fs::write(dir.join(format!("agentic-{w}x{h}.txt")), row_dump(terminal.backend().buffer())).unwrap();
        }
    }

    /// 折叠态给一行「改了什么」，展开态画着色的 diff；两者都要看得见文件与增删。
    #[test]
    fn edit_tools_show_counts_when_folded_and_the_diff_when_expanded() {
        let mut a = conversation_app();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let folded = row_dump(terminal.backend().buffer());
        assert!(folded.contains("src/lib.rs"), "{folded}");
        assert!(folded.contains("+1 −1"), "折叠态要有增删计数：{folded}");
        assert!(!folded.contains("+ let b = 3;"), "折叠态不画 diff 行：{folded}");

        a.expanded
            .insert("session:s1".into(), true);
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let expanded = row_dump(terminal.backend().buffer());
        assert!(expanded.contains("let b = 3;") || expanded.contains("b = 3;"), "{expanded}");
    }

    /// 信息栏只显示真实数据：没有 git 就没有「分支」那一行。
    #[test]
    fn the_sidebar_shows_real_data_and_omits_what_it_does_not_have() {
        let mut a = conversation_app();
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let text = row_dump(terminal.backend().buffer());
        assert!(text.contains("main · 2 个改动"), "{text}");
        assert!(text.contains("测试会话"), "标题在信息栏顶上：{text}");
        assert!(text.contains("改动的文件") && text.contains("src/lib.rs"), "{text}");
        assert!(!text.contains("状态") && !text.contains("授权"), "输入框已经有的不再写一遍：{text}");

        a.git = None;
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let text = row_dump(terminal.backend().buffer());
        assert!(!text.contains("main"), "没有 git 时整块省略，不写占位：{text}");
    }

    /// 审批弹层要压在最上面，且把「等谁 / 怎么答」写清楚。
    #[test]
    fn the_approval_dialog_names_the_tool_and_the_keys() {
        let mut a = conversation_app();
        let request = arc_deck_core::conversation::ApprovalRequest {
            request_id: "e1".into(),
            tool_name: "bash".into(),
            call_id: Some("call-1".into()),
            reason: Some("sandbox 提权请求".into()),
            display_reason: None,
            agent: Some("agent-1".into()),
        };
        a.approvals.push(request.clone());
        a.dialog = Some(Dialog::Approval(Box::new(request)));
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let text = row_dump(terminal.backend().buffer());
        assert!(text.contains("bash") && text.contains("审批"), "{text}");
        assert!(text.contains("Esc"), "要说清 Esc 不是允许：{text}");
    }

    /// 缓存：同一内容第二次绘制不重排全文；内容变了才重排。
    #[test]
    fn the_body_is_only_rebuilt_when_it_changed() {
        let mut a = conversation_app();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let builds = a.render_builds;
        assert_eq!(builds, 1);
        a.scroll = Some(0);
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        assert_eq!(a.render_builds, builds, "滚动不该重排全文");

        // 换主题：缓存里的行带着样式，必须重排。
        a.preferences.light = true;
        a.apply_preferences_theme();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        assert!(a.render_builds > builds, "换主题要重排正文");
        let builds = a.render_builds;

        // 一轮新的内容：必须重排。
        a.update(Update::Session(arc_deck_core::service::Event::Conversation {
            conversation: Box::new(arc_deck_core::conversation::Conversation {
                session_id: "s1".into(),
                ..arc_deck_core::conversation::Conversation::default()
            }),
        }));
        a.session_id = Some("s1".into());
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        assert!(a.render_builds > builds, "内容变了要重排");
    }

    /// 会话很长时，第二次绘制不再重排 —— 这就是「渲染只处理可见区域」的可测形式。
    #[test]
    fn a_long_conversation_is_not_rebuilt_on_every_frame() {
        use arc_deck_core::conversation::{AssistantTurn, TextPart, TurnState};
        let mut a = app();
        a.session_id = Some("s1".into());
        let messages: Vec<Message> = (1..=500)
            .map(|turn| {
                Message::Assistant(AssistantTurn {
                    turn,
                    blocks: vec![ViewBlock::Text(TextPart {
                        text: format!("第 {turn} 条回复：这是一段足够长的中文正文，用来让每一条都占好几行。"),
                        streaming: false,
                        step: 1,
                    })],
                    state: TurnState::Completed,
                    ..AssistantTurn::default()
                })
            })
            .collect();
        a.update(Update::Session(arc_deck_core::service::Event::Conversation {
            conversation: Box::new(arc_deck_core::conversation::Conversation {
                session_id: "s1".into(),
                messages,
                history_complete: true,
                ..Default::default()
            }),
        }));
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        assert_eq!(a.render_builds, 1);
        for _ in 0..5 {
            terminal.draw(|f| draw(f, &mut a)).unwrap();
        }
        assert_eq!(a.render_builds, 1, "500 条消息的会话在重复绘制时不该重排");
    }

    /// Enter 发送是 §7.4 与 OpenCode 的行为；F6 只是兼容别名，不能再出现在提示里
    /// （用户 2026-10-20：「哪个神人会用 F6 发送」）。
    #[test]
    fn the_footer_says_enter_sends() {
        let mut a = app();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let text = row_dump(terminal.backend().buffer());
        assert!(text.contains("Enter 发送"), "{text}");
        assert!(text.contains("Shift+Enter"), "{text}");
        assert!(!text.contains("F6"), "F6 不该再出现在提示里：{text}");
    }

    /// 页脚只有一行；长通知被截断，键位不被挤掉；普通通知 8 秒后收起。
    #[test]
    fn the_footer_is_one_row_and_a_long_notice_does_not_push_out_the_keys() {
        let mut a = app();
        a.notice = "授权缺口：".to_string() + &"很长的说明".repeat(40);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let buffer = terminal.backend().buffer();
        let last = row_text(buffer, 29);
        assert!(last.contains("授权缺口") && last.contains("…"), "{last}");
        assert!(last.contains("ctrl+p 命令"), "{last}");
        assert!(!row_dump(buffer).contains("PgUp 上滚"), "说明书那一行不在了");

        a.tick();
        a.notice_seen_at_for_test(std::time::Instant::now() - std::time::Duration::from_secs(9));
        a.tick();
        assert!(a.notice.is_empty(), "普通通知到期收起");
    }

    /// 三个 Agent 在**每个**内置主题、两种模式下都是三种不同的颜色；默认主题里 Echo 是青色。
    #[test]
    fn agents_have_distinct_colours_in_every_theme_and_echo_is_cyan() {
        for name in crate::theme::theme_names() {
            for mode in [crate::theme::Mode::Dark, crate::theme::Mode::Light] {
                let theme = Theme::load(name, mode).unwrap();
                let [echo, build, plan] = agent_palette(&theme);
                assert!(echo != build && build != plan && echo != plan, "{name} ({mode:?}) 撞色");
            }
        }
        let theme = Theme::load(crate::theme::DEFAULT_THEME, crate::theme::Mode::Dark).unwrap();
        assert_eq!(agent_palette(&theme)[0], theme.hue("cyan", 200).unwrap());
    }

    /// 输入框有自己的底色（视觉焦点），左边竖线是 Agent 的颜色，下沿是一排 `▀`。
    #[test]
    fn the_composer_has_its_own_background() {
        let mut a = app();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let buffer = terminal.backend().buffer();
        let page = a.theme.background.to_ratatui();
        let (x, y) = (0..30)
            .flat_map(|y| (0..100).map(move |x| (x, y)))
            .find(|&(x, y)| buffer[(x, y)].symbol() == "┃")
            .expect("输入框的竖线");
        assert_eq!(buffer[(x, y)].fg, agent_color(&a, &a.theme));
        let inside = &buffer[(x + 5, y)];
        assert_ne!(inside.bg, page, "输入框底色要和页面分得开");
        assert_eq!(inside.bg, prompt_bg(&a.theme));
        assert!((y..30).any(|row| buffer[(x + 3, row)].symbol() == "▀"), "下沿是半格");
    }

    /// 在干活时页脚左边是会动的扫描条 + `esc 中断`；空闲时没有。重画只在干活时按帧触发。
    #[test]
    fn the_scanner_runs_only_while_working() {
        let color = crate::color::Rgb::opaque(0x56, 0xb6, 0xc2);
        let base = Color::Rgb(0, 0, 0);
        let text = |frame| scanner(frame, color, base).iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(0), "■⬝⬝⬝⬝⬝⬝⬝", "第一帧亮头在最左");
        assert!(text(7).ends_with('■'), "走到最右");
        assert_ne!(text(3), text(4), "会动");

        let mut a = app();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        assert!(!row_text(terminal.backend().buffer(), 29).contains("esc 中断"));
        a.pending = Some("你好".into());
        assert!(a.working());
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let footer = row_text(terminal.backend().buffer(), 29);
        assert!(footer.contains("esc 中断") && footer.contains('■'), "{footer}");
        a.dirty = false;
        std::thread::sleep(std::time::Duration::from_millis(60));
        a.tick();
        assert!(a.dirty, "干活时扫描条按帧重画");
        a.pending = None;
        a.dirty = false;
        std::thread::sleep(std::time::Duration::from_millis(60));
        a.tick();
        assert!(!a.dirty, "空闲时不为扫描条重画");
    }

    /// 照 OpenCode v1：用户消息是左边一条竖线 + 抬高的背景，没有「你」标签；
    /// 助手正文也不带角色标签。
    #[test]
    fn user_messages_are_a_bordered_block_without_role_labels() {
        let a = conversation_app();
        let (lines, offsets) = conversation_lines(&a, &a.theme.clone(), 80);
        assert!(!offsets.is_empty());
        assert!(lines[0].to_string().starts_with('┃'), "{:?}", lines[0].to_string());
        assert_eq!(lines[0].spans[0].style.fg, Some(agent_color(&a, &a.theme)), "竖线是 Agent 的颜色");
        assert!(
            lines.iter().all(|line| line.to_string() != "你"),
            "不该有「你」这一行"
        );
        assert!(
            lines.iter().all(|line| line.to_string() != "Saiku"),
            "助手正文不该有角色标签"
        );
        // 用户块的行尾留白也带背景色，才像一块面板。
        assert_eq!(lines[0].spans.last().unwrap().style.bg, Some(panel_deep(&a.theme)));
    }

    /// 带语言的代码块要按主题 `syntax.*` 上色；认不出的语言保持单色。
    #[test]
    fn code_blocks_use_the_theme_syntax_colors() {
        use crate::markdown;
        let a = app();
        let palette = markdown_palette(&a.theme);
        let rendered = markdown::render("```rust\nlet x = 1; // hi\n```", 60, &palette);
        let colors: std::collections::HashSet<Option<ratatui::style::Color>> = rendered
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.style.fg))
            .collect();
        assert!(
            colors.len() >= 3,
            "关键字 / 数字 / 注释要不同色：{colors:?}"
        );
    }

    fn row_dump(buffer: &ratatui::buffer::Buffer) -> String {
        let mut text = String::new();
        for y in 0..buffer.area.height {
            text.push_str(&row_text(buffer, y));
            text.push('\n');
        }
        text
    }
}
