// Arc Deck · IME 适配层
//
// 分两块，边界是刻意的：
//
// - [`ImeState`] —— **纯逻辑状态机**，零平台依赖，可单测。它的职责是把
//   「组合中 → 提交」的生命周期纪律固化下来：组合串不得进入文档、不得进入
//   撤销栈、不得改动选区。
// - [`ImeSurface`]（`cfg(windows)`）—— IMM32 绑定，把 `WM_IME_*` 翻译成
//   [`crate::input::InputEvent`]。
//
// 分开的原因：2026 年所有纯 Rust GUI 框架的中文 IME 都有未关闭的缺陷，而那些
// 缺陷多半出在**生命周期纪律**上（组合期间丢字、失焦丢最后一个字、组合串污染
// 文档）—— 与操作系统绑定无关。把这部分做成纯逻辑，就能用测试钉死，
// 而不是靠手工点窗口发现问题。

use crate::input::InputEvent;

/// IME 组合状态机 —— 平台无关。
///
/// 不变量（每条都有对应测试）：
/// 1. 组合串只存在于 [`ImeState::preedit`]，**永远不进入文档**
/// 2. 提交串是唯一会产出 [`InputEvent::Commit`] 的路径
/// 3. 重新开始组合会**清空**上一次的组合串（不累积）
/// 4. 取消组合（失焦 / Esc）清空组合串但**不产出提交**
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ImeState {
    /// 当前组合串（预编辑文本）。空表示不在组合中。
    preedit: String,
    /// 组合串内的光标偏移（以**字符**计，非字节）。
    cursor: Option<usize>,
    /// 组合串的来源序号 —— 用于丢弃过期的 `done` 批次。
    ///
    /// fcitx5 与 IMM32 都会出现「陈旧帧」：组合已经推进，但仍有一条早先的
    /// 组合串到达。用单调序号过滤是这两个平台共同的必要手段。
    generation: u64,
}

impl ImeState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前组合串。空串表示不在组合中。
    pub fn preedit(&self) -> &str {
        &self.preedit
    }

    /// 是否正在组合。
    pub fn is_composing(&self) -> bool {
        !self.preedit.is_empty()
    }

    /// 组合串内的光标（字符计）。
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// 当前组合世代。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 组合开始 —— 清空上一轮残留。
    ///
    /// 不产出事件：组合开始本身不改动任何可见文本。
    pub fn on_composition_start(&mut self) {
        self.preedit.clear();
        self.cursor = None;
        self.generation += 1;
    }

    /// 组合更新 —— 设置当前组合串。
    ///
    /// `cursor_byte` 为**字节**偏移（IMM32 `GCS_CURSORPOS` 与 Wayland
    /// `preedit_string` 都以此为约定），内部换算为字符偏移以防 UTF-8 边界
    /// 被切坏 —— 这正是多数框架丢字/乱码的根因。
    pub fn on_composition_update(&mut self, text: String, cursor_byte: Option<i32>) {
        self.cursor = cursor_byte.and_then(|b| byte_to_char_index(&text, b));
        self.preedit = text;
    }

    /// 组合提交 —— 产出唯一会进入文档的事件。
    ///
    /// 提交后组合串必定被清空：这是「提交即终结本世代」的纪律。
    pub fn on_commit(&mut self, text: String) -> Option<InputEvent> {
        self.preedit.clear();
        self.cursor = None;
        if text.is_empty() {
            // 空提交是合法的 no-op（某些 IME 用它表示取消），不产出事件。
            return None;
        }
        Some(InputEvent::Commit { text })
    }

    /// 组合结束（失焦 / Esc / 窗口切换）。
    ///
    /// **清空组合串但不产出提交** —— 这是「失焦不得丢字」与「失焦不得误提交」
    /// 两条约束的交点：未提交的组合本来就该丢弃，而已提交的文本早就在文档里。
    pub fn on_composition_end(&mut self) {
        self.preedit.clear();
        self.cursor = None;
    }

    /// 供渲染使用：产出 preedit 事件。`None` 表示无组合，App 无需画下划线。
    pub fn as_preedit_event(&self) -> Option<InputEvent> {
        if self.preedit.is_empty() {
            None
        } else {
            Some(InputEvent::Preedit {
                text: self.preedit.clone(),
                cursor: self.cursor,
            })
        }
    }
}

/// 字节偏移 → 字符偏移。越界或落在多字节字符中间时，向外夹紧到最近的字符边界。
///
/// 返回 `None` 仅当偏移为负。
fn byte_to_char_index(s: &str, byte: i32) -> Option<usize> {
    if byte < 0 {
        return None;
    }
    let target = byte as usize;
    let mut count = 0usize;
    for (idx, _) in s.char_indices() {
        if idx >= target {
            return Some(count);
        }
        count += 1;
    }
    // 偏移落在串尾或之后 —— 夹紧到末尾。
    Some(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preedit_never_becomes_a_commit() {
        // 不变量 1：组合串不得进入文档。反复更新组合串期间，不应出现任何 Commit。
        let mut st = ImeState::new();
        st.on_composition_start();
        st.on_composition_update("z".into(), None);
        st.on_composition_update("zh".into(), None);
        st.on_composition_update("zho".into(), None);
        assert!(st.is_composing());
        assert_eq!(st.preedit(), "zho");
        assert!(matches!(st.as_preedit_event(), Some(InputEvent::Preedit { .. })));
    }

    #[test]
    fn commit_clears_preedit_and_yields_exactly_one_event() {
        // 不变量 2 + 提交即终结世代。
        let mut st = ImeState::new();
        st.on_composition_start();
        st.on_composition_update("zhong".into(), None);
        let ev = st.on_commit("中".into());
        assert_eq!(ev, Some(InputEvent::Commit { text: "中".into() }));
        assert!(!st.is_composing(), "提交后必须清空组合串");
        assert!(st.as_preedit_event().is_none(), "提交后不得再产出 preedit");
    }

    #[test]
    fn restart_does_not_accumulate() {
        // 不变量 3：重新组合清空上一轮，不累积。
        let mut st = ImeState::new();
        st.on_composition_start();
        st.on_composition_update("first".into(), None);
        st.on_composition_start();
        assert_eq!(st.preedit(), "");
        st.on_composition_update("sec".into(), None);
        assert_eq!(st.preedit(), "sec");
    }

    #[test]
    fn cancel_clears_without_committing() {
        // 不变量 4：取消清空但不提交 —— 失焦丢弃未提交组合是正确的。
        let mut st = ImeState::new();
        st.on_composition_start();
        st.on_composition_update("zhong".into(), None);
        st.on_composition_end();
        assert!(!st.is_composing());
        assert!(st.as_preedit_event().is_none());
    }

    #[test]
    fn empty_commit_is_a_silent_noop() {
        let mut st = ImeState::new();
        st.on_composition_start();
        assert_eq!(st.on_commit(String::new()), None);
    }

    #[test]
    fn generation_advances_on_each_composition_start() {
        let mut st = ImeState::new();
        let g0 = st.generation();
        st.on_composition_start();
        assert!(st.generation() > g0);
        let g1 = st.generation();
        st.on_composition_start();
        assert!(st.generation() > g1);
    }

    #[test]
    fn cursor_byte_maps_to_char_index_for_cjk() {
        // 关键回归：组合串含中文时，字节偏移与字符偏移不等。
        // 「中」占 3 字节 —— GCS_CURSORPOS 给 3 时字符位置应为 1。
        assert_eq!(byte_to_char_index("中", 0), Some(0));
        assert_eq!(byte_to_char_index("中", 3), Some(1));
        assert_eq!(byte_to_char_index("中文", 6), Some(2));
    }

    #[test]
    fn cursor_byte_clamps_inside_multibyte_char() {
        // 落在「中」的中间字节(1)上 —— 必须夹紧到边界，不得切坏 UTF-8。
        let idx = byte_to_char_index("中", 1).unwrap();
        assert!(idx <= 1, "must snap to a char boundary, got {idx}");
    }

    #[test]
    fn cursor_byte_beyond_end_clamps_to_len() {
        assert_eq!(byte_to_char_index("ab", 999), Some(2));
    }

    #[test]
    fn negative_cursor_is_rejected() {
        assert_eq!(byte_to_char_index("ab", -1), None);
    }
}
