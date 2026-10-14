# 任务书 w1-theme：语义主题系统（2.0.27 token）与主题选择

（先读同目录 `_common.md` 的通用约定，它的全部条款适用于本任务。路径：
`C:/Users/mc158/Documents/Projects/arc-deck/tasks/m4/_common.md`）

## 背景

`crates/arc-deck-tui/src/theme.rs` 现在是一个手写的深/浅两套扁平颜色。PRODUCT_PLAN §7.1 / §7.3 定的是：
**主题 token 取 OpenCode 2.0.27 的语义集合**（text / background / action / formfield / feedback / border，
含 hover / selected / disabled 等状态角色），并能用 OpenCode 的内置主题。

参考（`git show v2.0.27:<路径>`）：
- `packages/theme/src/tui/schema.ts`（语义 token 的全集与形状）
- `packages/tui/src/theme/resolve.ts`、`color.ts`、`component.ts`、`index.ts`、`discovery.ts`
- `packages/tui/src/theme/v1.ts`（**v1 扁平调色板 → v2 语义 token 的迁移**，这是关键）
- `packages/tui/src/theme/assets/*.json`（v1 主题）与 `assets/v2/opencode.json`（v2 默认主题）
- `packages/tui/src/component/dialog-theme-list.tsx`、`packages/tui/src/context/theme.tsx`

## 要做的

1. 用 Rust 表达 2.0.27 的语义 token 集合（含状态角色），以及解析出最终颜色的过程（resolve）。
   **另外保留 markdown 与 syntax 两组 token**（v1 主题里的 `markdown*` / `syntax*` 键，v2 里的对应项）——
   另一个工人在做 Markdown 渲染，合并后要从这里取色。字段命名用 snake_case，如 `markdown_heading`。
2. 内置主题：把 v2.0.27 的 `theme/assets/` 下全部主题 JSON 拷进 `crates/arc-deck-tui/themes/`
   （保持原文，MIT notice 已在 `crates/arc-deck-tui/LICENSE-OpenCode`；在 themes 目录放一个 README 说明来源与 tag），
   用 `include_str!` 编进二进制。v1 主题经 v1→v2 迁移后使用，与上游 `v1.ts` 行为一致。
   每个主题有深/浅两种取值（JSON 里 `{dark, light}` 的写法与 `defs` 引用都要支持）。
3. 默认主题：OpenCode 的默认（`opencode`），深色。**现有画面观感不能变差** —— 现有 `Theme` 的每个字段都要
   映射到新 token 上；`view.rs` 等调用方可以改用新的字段名。终端只支持 16/256 色时的降级：如果上游有
   处理就照着做，没有就保持 RGB 并在最终回复里说明。上游的 `system`（读终端调色板）主题本任务不做，写进未做项。
4. 主题选择对话框（照 `dialog-theme-list.tsx`）：`/theme` 与命令面板里的「主题」打开一个**可搜索**列表；
   ↑/↓ 移动时**实时预览**，Enter 确认，Esc 恢复打开前的主题。另有一个动作切换深/浅模式（保留现有
   `/theme` 的那个深浅切换功能，但改名为 `/mode`，命令注册表 `COMMANDS` 里同步）。
   **对话框的渲染与状态写在新文件里**（如 `theme_dialog.rs`），`app.rs` / `view.rs` 只加最少的接线 ——
   另一个工人同时在改模型对话框，这样合并冲突最小。
5. 持久化：所选主题名与深浅模式存进现有的 `Preferences`（`tui-preferences.json`），旧文件（只有深浅字段）
   要能读，不认识的主题名回退到默认并且**不崩**。
6. 测试：v1→v2 迁移对 2～3 个代表性主题的若干 token 取值与上游逻辑一致（在测试注释里写出你据以计算的
   上游规则）；`defs` 引用与深浅取值；全部内置主题都能解析、不 panic；对话框的预览 / 确认 / Esc 恢复；
   旧偏好文件兼容；「正常文字不使用 error 色」这类既有断言继续成立。
