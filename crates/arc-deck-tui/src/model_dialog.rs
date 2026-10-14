//! 模型 / provider / effort 三个对话框。
//!
//! 移植自 OpenCode v1.19.0（MIT，notice: ../LICENSE-OpenCode）：
//! `component/dialog-model.tsx`（分节、Favorites/Recent、当前项标记、Free 页脚、
//! 选中后按需进入强度选择）、`component/dialog-provider.tsx`（独立的 provider 列表入口）、
//! `component/dialog-variant.tsx`（强度列表 + Default 一项）、`ui/dialog-select.tsx`
//! （搜索行、分节标题、禁用行、底部动作提示、`esc` 提示）。
//!
//! 三处有意与上游不同：
//! 1. **不做 provider 的连接 / 登录**。上游的 provider 对话框是 OAuth 与 API key 流程的入口，
//!    Arc Deck 的 provider 与凭据来自用户配置与 `auth.json`（`arcdeck auth set`），
//!    在 TUI 里再做一个登录流程等于第二套真相。这里只做列表与跳转。
//! 2. **当前项标记用 `●` 而不是只把文字染色**：终端的配色能力不确定，
//!    而「哪个模型现在是当前」是这一屏最重要的信息，不该依赖颜色。
//! 3. **禁用行留在列表里，只在确认那一步拒绝**。上游把禁用项整个过滤掉
//!    （`filter(x => x.disabled !== true)`），而 §7.2 要求「不可用的模型显式禁用并说明原因，
//!    不是从列表里悄悄消失」。

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    catalog::{self, ModelOption, ModelPrefs, ModelRef, Provider},
    theme::Theme,
};

/// 模型对话框的来源。`only_provider` 是从 provider 列表跳进来的那种。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModelDialogState {
    pub only_provider: Option<usize>,
}

impl ModelDialogState {
    pub fn provider<'a>(&self, providers: &'a [Provider]) -> Option<&'a Provider> {
        self.only_provider.and_then(|index| providers.get(index))
    }
    pub fn provider_id<'a>(&self, providers: &'a [Provider]) -> Option<&'a str> {
        self.provider(providers).map(|provider| provider.id.as_str())
    }
}

/// provider 列表的状态。`back` 是跳进来的那个模型对话框 ——
/// Esc 要能回到它（上游 `DialogProvider` 是从模型对话框 `replace` 进去的），
/// 否则「从 provider 列表进来的模型列表」这条路上的 Esc 会直接把整个流程关掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProviderDialogState {
    pub back: Option<ModelDialogState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffortDialogState {
    /// 选定 effort 时作用在哪个模型上 —— 必须钉住，不能用「当时的当前模型」：
    /// 对话框开着的时候模型可能已经被 F2 换掉了。
    pub model: ModelRef,
    /// 这个模型报出来的档数，只用于计数（档名来自目录，这里不复述一遍）。
    pub efforts: usize,
}

/// 模型列表里的一行。选中、禁用与展示都按它判断，所以渲染与按键读同一份数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRow {
    pub model: ModelRef,
    pub title: String,
    pub description: Option<String>,
    pub footer: Option<String>,
    pub disabled: Option<String>,
    pub current: bool,
    pub efforts: usize,
}

/// 一行「标题 + 行」。标题为空表示这一节不画标题（搜索命中、限定 provider）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelBlock {
    pub title: String,
    pub rows: Vec<ModelRow>,
}

/// 把目录的分节翻成界面行。`current` 与 `efforts` 只有目录知道，所以在这里补齐。
pub fn blocks(
    providers: &[Provider],
    prefs: &ModelPrefs,
    query: &str,
    only_provider: Option<&str>,
    current: Option<&ModelRef>,
) -> Vec<ModelBlock> {
    catalog::sections(providers, prefs, query, only_provider)
        .into_iter()
        .map(|section| ModelBlock {
            title: section.title,
            rows: section
                .options
                .into_iter()
                .map(|option| row(providers, option, current))
                .collect(),
        })
        .collect()
}

fn row(providers: &[Provider], option: ModelOption, current: Option<&ModelRef>) -> ModelRow {
    let efforts = catalog::lookup_route(providers, &option.model)
        .map(|(_, model)| model.efforts.len())
        .unwrap_or(0);
    ModelRow {
        current: current == Some(&option.model),
        model: option.model,
        title: option.title,
        description: option.description,
        footer: option.footer,
        disabled: option.disabled,
        efforts,
    }
}

/// 平铺成一维，供上下选择用。分节标题不占选中位置。
pub fn flatten(blocks: &[ModelBlock]) -> Vec<ModelRow> {
    blocks
        .iter()
        .flat_map(|block| block.rows.iter().cloned())
        .collect()
}

/// 当前对话框状态下的候选行。
pub fn rows(
    providers: &[Provider],
    prefs: &ModelPrefs,
    state: ModelDialogState,
    query: &str,
    current: Option<&ModelRef>,
) -> Vec<ModelRow> {
    flatten(&blocks(
        providers,
        prefs,
        query,
        state.provider_id(providers),
        current,
    ))
}

/// 列表里第 `choice` 行。越界返回 `None`（搜索让列表变短时的正常情形）。
pub fn selected(
    providers: &[Provider],
    prefs: &ModelPrefs,
    state: ModelDialogState,
    query: &str,
    current: Option<&ModelRef>,
    choice: usize,
) -> Option<ModelRow> {
    rows(providers, prefs, state, query, current)
        .into_iter()
        .nth(choice)
}

/// effort 对话框的候选：一个「默认」+ 模型报出来的强度档。
/// 返回 `None` 表示这个模型没有强度档，选中它时不该进这个对话框（§7.2）。
pub fn efforts(providers: &[Provider], model: &ModelRef) -> Option<Vec<String>> {
    let (_, found) = catalog::lookup_route(providers, model)?;
    (!found.efforts.is_empty()).then(|| found.efforts.clone())
}

/// effort 对话框里第 `choice` 行对应的取值：`None` 是「默认」那一行。
/// 越界也返回 `None`，与「默认」同一档 —— 调用方不必特判列表正在变短。
pub fn effort_at(
    providers: &[Provider],
    state: &EffortDialogState,
    choice: usize,
) -> Option<String> {
    if choice == 0 {
        return None;
    }
    efforts(providers, &state.model).and_then(|list| list.get(choice - 1).cloned())
}

/// 模型对话框这一帧要看的东西。打成一个结构体是因为它已经有 6 个字段 ——
/// 逐个传参时调用点会长到看不出哪一项对应哪一项。
#[derive(Clone, Copy)]
pub struct ModelView<'a> {
    pub providers: &'a [Provider],
    pub prefs: &'a ModelPrefs,
    pub state: ModelDialogState,
    pub query: &'a str,
    pub choice: usize,
    pub current: Option<&'a ModelRef>,
}

/// provider 列表这一帧要看的东西。
#[derive(Clone, Copy)]
pub struct ProviderView<'a> {
    pub providers: &'a [Provider],
    pub query: &'a str,
    pub choice: usize,
    pub current: Option<&'a ModelRef>,
}

/// effort 列表这一帧要看的东西。`state` 为 `None` 表示当前没有模型（只画「默认」）。
#[derive(Clone, Copy)]
pub struct EffortView<'a> {
    pub providers: &'a [Provider],
    pub state: Option<&'a EffortDialogState>,
    pub choice: usize,
    pub current: Option<&'a str>,
}

/// 画模型对话框。返回搜索行的位置，调用方据此放光标。
pub fn draw_model(frame: &mut Frame, area: Rect, t: &Theme, view: ModelView<'_>) -> Rect {
    let ModelView {
        providers,
        prefs,
        state,
        query,
        choice,
        current,
    } = view;
    let rect = geometry(area);
    let inner = frame_in(frame, rect, title_for(t, providers, state), t);
    let search = Rect::new(inner.x, inner.y, inner.width, 1);
    // 「下一次请求生效」必须写在对话框里：worker 的路由在 initialize 时就被钉住（§7.2），
    // 「以为换完了、实际这一轮还在用旧路由」是这一类界面最容易造成的误解。
    // 这行文字要压在一行里（内宽 72 列）—— 挤成两行时对话框的高度与
    // 「Esc 关闭」那一句都会被顶掉，实测过。
    frame.render_widget(
        Paragraph::new(Line::styled(
            "切换模型对下一次请求生效，运行中不会热切换",
            Style::default().fg(crate::view::warning(t)),
        )),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    let list = Rect::new(
        inner.x,
        inner.y + 3,
        inner.width,
        inner.height.saturating_sub(4),
    );
    let blocks = blocks(providers, prefs, query, state.provider_id(providers), current);
    let chosen_line = choice_line(&blocks, choice);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (nth, block) in blocks.iter().enumerate() {
        if !block.title.is_empty() {
            if nth > 0 {
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(
                block.title.clone(),
                Style::default().fg(crate::view::accent(t)).add_modifier(Modifier::BOLD),
            ));
        }
        for row in &block.rows {
            let active = chosen_line == Some(lines.len());
            lines.push(model_line(row, active, inner.width, t));
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "没有匹配的模型",
            Style::default().fg(crate::view::muted(t)),
        ));
    }
    let (start, shown) = window(lines.len(), chosen_line.unwrap_or(0), list.height as usize);
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(start).take(shown).collect::<Vec<_>>()),
        list,
    );

    let close = current
        .map(|model| format!(" · 仍用 {}", catalog::display_name(providers, model)))
        .unwrap_or_default();
    footer(
        frame,
        inner,
        &format!("^F 收藏 · ^A provider · Enter 选定{close} · Esc 关闭"),
        t,
    );
    Rect::new(search.x, search.y, search.width.saturating_sub(1), 1)
}

pub fn draw_providers(frame: &mut Frame, area: Rect, t: &Theme, view: ProviderView<'_>) -> Rect {
    let ProviderView {
        providers,
        query,
        choice,
        current,
    } = view;
    let rect = geometry(area);
    let inner = frame_in(frame, rect, headline(t, "选择 provider"), t);
    let search = Rect::new(inner.x, inner.y, inner.width, 1);
    let list = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(3),
    );
    let needle = query.trim().to_lowercase();
    let options: Vec<catalog::ProviderOption> =
        provider_candidates(providers, current, &needle);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (index, option) in options.iter().enumerate() {
        let active = index == choice;
        let style = if active {
            Style::default()
                .fg(crate::view::row_focus(t).0)
                .bg(crate::view::row_focus(t).1)
                .add_modifier(Modifier::BOLD)
        } else if option.current {
            Style::default().fg(crate::view::accent(t))
        } else {
            Style::default().fg(crate::view::text(t))
        };
        lines.push(Line::styled(
            fill(
                format!("{} {}", marker(option.current), option.name),
                option.description.clone(),
                inner.width,
            ),
            style,
        ));
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "没有匹配的 provider",
            Style::default().fg(crate::view::muted(t)),
        ));
    }
    let (start, shown) = window(lines.len(), choice, list.height as usize);
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(start).take(shown).collect::<Vec<_>>()),
        list,
    );
    footer(
        frame,
        inner,
        "↑↓ 选择 · Enter 打开该 provider 的模型 · Esc 返回模型对话框",
        t,
    );    Rect::new(search.x, search.y, search.width.saturating_sub(1), 1)
}

/// provider 列表的候选行（对话框与 `App` 的选中判断共用一份顺序）。
pub fn provider_rows(
    providers: &[Provider],
    current: Option<&ModelRef>,
) -> Vec<catalog::ProviderOption> {
    catalog::provider_options(providers, current)
}

/// 搜索过滤后的 provider 行。**渲染与确认必须走同一个函数** ——
/// 否则搜索把列表缩短之后，选中的下标在 `App` 那边对的是另一份列表，
/// Enter 落到的是没被过滤掉的那一条后面的 provider（实测踩到过）。
pub fn provider_candidates(
    providers: &[Provider],
    current: Option<&ModelRef>,
    needle: &str,
) -> Vec<catalog::ProviderOption> {
    provider_rows(providers, current)
        .into_iter()
        .filter(|option| {
            needle.is_empty()
                || format!("{} {}", option.id, option.name)
                    .to_lowercase()
                    .contains(needle)
        })
        .collect()
}

pub fn draw_efforts(frame: &mut Frame, area: Rect, t: &Theme, view: EffortView<'_>) -> Rect {
    let EffortView {
        providers,
        state,
        choice,
        current,
    } = view;
    let rect = geometry(area);
    let model = state
        .map(|state| catalog::display_name(providers, &state.model))
        .unwrap_or_else(|| "模型".into());
    let inner = frame_in(frame, rect, headline(t, &format!("选择 effort · {model}")), t);
    let list = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(2),
    );
    let mut entries: Vec<(String, Option<String>)> =
        vec![("默认".into(), Some("跟随模型自己的缺省强度".into()))];
    if let Some(state) = state
        && let Some((_, found)) = catalog::lookup_route(providers, &state.model)
    {
        entries.extend(found.efforts.iter().map(|effort| (effort.clone(), None)));
    }
    let selected = current.unwrap_or("默认");
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (index, (title, description)) in entries.iter().enumerate() {
        let active = index == choice;
        let is_current = title == selected;
        let style = if active {
            Style::default()
                .fg(crate::view::row_focus(t).0)
                .bg(crate::view::row_focus(t).1)
                .add_modifier(Modifier::BOLD)
        } else if is_current {
            Style::default().fg(crate::view::accent(t))
        } else {
            Style::default().fg(crate::view::text(t))
        };
        lines.push(Line::styled(
            fill(
                format!("{} {}", marker(is_current), title),
                description.clone(),
                inner.width,
            ),
            style,
        ));
    }
    let (start, shown) = window(lines.len(), choice, list.height as usize);
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(start).take(shown).collect::<Vec<_>>()),
        list,
    );
    footer(
        frame,
        inner,
        "↑↓ 选择 · Enter 选定（下一次请求带 --effort）· 默认恢复模型缺省值",
        t,
    );
    // effort 没有搜索行，返回零宽矩形。
    Rect::new(inner.x, inner.y, 0, 1)
}

// ── 渲染零件 ──────────────────────────────────────────────────────────

fn title_for(t: &Theme, providers: &[Provider], state: ModelDialogState) -> Line<'static> {
    match state.provider(providers) {
        Some(provider) => Line::from(vec![
            Span::styled(
                "选择模型",
                Style::default().fg(crate::view::text(t)).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  ·  {}", provider.name),
                Style::default().fg(crate::view::muted(t)),
            ),
        ]),
        None => headline(t, "选择模型"),
    }
}

fn headline(t: &Theme, text: &str) -> Line<'static> {
    Line::styled(
        text.to_string(),
        Style::default().fg(crate::view::text(t)).add_modifier(Modifier::BOLD),
    )
}

fn frame_in(frame: &mut Frame, rect: Rect, title: Line<'static>, t: &Theme) -> Rect {
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::new()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(t.border.to_ratatui()))
            .style(Style::default().bg(crate::view::dialog_bg(t))),
        rect,
    );
    Rect::new(
        rect.x + 2,
        rect.y + 1,
        rect.width.saturating_sub(4),
        rect.height.saturating_sub(2),
    )
}

fn geometry(area: Rect) -> Rect {
    // 一律 saturating_sub：窄终端（40 列）下普通减法会在 debug 构建里 panic，
    // 而「窗口太小」正是这条路径最可能出现的情形之一。
    let width = 76.min(area.width.saturating_sub(2));
    let height = 22.min(area.height.saturating_sub(2)).max(8);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn model_line(row: &ModelRow, active: bool, width: u16, t: &Theme) -> Line<'static> {
    let disabled = row.disabled.is_some();
    let style = if active && !disabled {
        Style::default()
            .fg(crate::view::row_focus(t).0)
            .bg(crate::view::row_focus(t).1)
            .add_modifier(Modifier::BOLD)
    } else if active {
        // 禁用且被选中：仍然要看起来是禁用的，只把光标那一行的底色画出来。
        Style::default().fg(crate::view::disabled(t)).bg(crate::view::row_focus(t).1)
    } else if disabled {
        Style::default().fg(crate::view::disabled(t))
    } else if row.current {
        Style::default().fg(crate::view::accent(t))
    } else {
        Style::default().fg(crate::view::text(t))
    };

    // 副标题：收藏标记与不可用原因，两者可能同时存在。
    let mut note = row.description.clone().unwrap_or_default();
    if let Some(reason) = &row.disabled {
        if !note.is_empty() {
            note.push(' ');
        }
        note.push_str(&format!("（不可用：{reason}）"));
    }
    let head = format!(" {} {}", marker(row.current), row.title);
    // footer（Free）右对齐；中间用空格把标题与页脚分开。
    let footer = row.footer.clone().unwrap_or_default();
    let pad = (width as usize)
        .saturating_sub(display_width(&head) + display_width(&note) + display_width(&footer) + 2)
        .max(1);
    Line::from(vec![
        Span::styled(head, style),
        Span::styled(
            format!("{note}{}", " ".repeat(pad)),
            if disabled {
                Style::default().fg(crate::view::disabled(t))
            } else {
                Style::default().fg(crate::view::muted(t))
            },
        ),
        Span::styled(
            footer,
            if active && !disabled {
                style
            } else {
                Style::default().fg(crate::view::muted(t))
            },
        ),
    ])
}

fn fill(left: String, right: Option<String>, width: u16) -> String {
    let right = right.unwrap_or_default();
    let pad = (width as usize)
        .saturating_sub(display_width(&left) + display_width(&right) + 1)
        .max(1);
    format!("{left}{}{right}", " ".repeat(pad))
}

fn marker(current: bool) -> &'static str {
    if current { "●" } else { " " }
}

fn footer(frame: &mut Frame, inner: Rect, text: &str, t: &Theme) {
    frame.render_widget(
        Paragraph::new(clean(text)).style(Style::default().fg(crate::view::muted(t))),
        Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
    );
}

/// 让选中行落在可视窗口里，尽量居中；返回（起始行、可见行数）。
fn window(total: usize, choice: usize, height: usize) -> (usize, usize) {
    let height = height.max(1);
    if total <= height {
        return (0, height);
    }
    let start = choice.saturating_sub(height / 2).min(total - height);
    (start, height)
}

/// 选中行对应的**渲染行**下标（分节标题与分节之间的空行各占一行）。
fn choice_line(blocks: &[ModelBlock], choice: usize) -> Option<usize> {
    let mut line = 0;
    let mut index = 0;
    for (nth, block) in blocks.iter().enumerate() {
        if !block.title.is_empty() {
            if nth > 0 {
                line += 1;
            }
            line += 1;
        }
        if choice < index + block.rows.len() {
            return Some(line + (choice - index));
        }
        line += block.rows.len();
        index += block.rows.len();
    }
    None
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}
