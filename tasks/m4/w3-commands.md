# 任务书 w3-commands：命令系统（action 注册表、键位、slash 补全、命令面板、帮助）

（先读 `C:/Users/mc158/Documents/Projects/arc-deck/tasks/m4/_common.md`，全部条款适用。）

## 背景

PRODUCT_PLAN §7.3「命令与选择」、§7.4（**先读全文**）。现状：`crates/arc-deck-tui/src/app.rs` 有一张 `COMMANDS` 表和
`Ctrl+P` 面板、`Ctrl+X` leader 的几个键，但键位是在 `handle_key` 里手写匹配的，slash 只在提交时识别，没有补全、没有帮助、
没有上下文解析，§7.4 列的命令也不全。

参考 v1.19.0：`config/keybind.ts`（**键位定义表与默认值的形状**）、`component/command-palette.tsx`、
`component/prompt/autocomplete.tsx`（`/` 补全；`@` 文件补全是另一件任务，但**补全弹层要做成通用的**）、
`ui/dialog-help.tsx`、`feature-plugins/system/which-key.tsx`（leader 按下后的提示）、`routes/session/index.tsx`
（会话页注册的命令）、`app.tsx`（全局命令）。

## 要做的

1. **一张 action 注册表是唯一出处**：每个 action 有 id、标题、分类、slash 名（可无）、默认键位（可多个、可无）、
   生效上下文（全局 / 输入区 / 某个对话框 / 运行中）、以及**可用性**（可用 / 不可用 + 原因）。命令面板、slash、键位、帮助、
   which-key 提示全部从这张表生成 —— 不许出现第二份标题或键位文字。
2. **键位解析**：照 `keybind.ts` 的写法支持 `ctrl+x`、`shift+tab`、`<leader>n`、逗号分隔多绑定、`none` 关闭。
   **按上下文解析**：对话框打开时对话框的键优先，同名的会话级绑定不触发（§7.4：Ctrl+F / Ctrl+A 在模型对话框内另有含义）。
   leader 有超时（照上游的值）与 which-key 提示（按下 leader 后在底部列出可接的键）。
   用户覆盖：读 `tui-preferences.json` 旁的 `keybinds.json`（action id → 键位字符串），非法值报出来并回退默认，不崩。
3. **§7.4 的命令全部在表里**：`/new` `/sessions` `/agents` `/models` `/effort` `/permission` `/plan` `/build` `/cancel`
   `/usage` `/theme` `/mode` `/sidebar` `/tools` `/latest` `/evidence` `/help` `/exit`，以及键位 `Ctrl+P`、`<leader>a/n/l/b/m/t`、
   `Shift+Tab`（Agent 切换）、`F2/Shift+F2`、`Esc` 的分层语义（先关弹层 → 再清编辑状态 → 无弹层且运行中才请求取消）。
   **后端还没有的能力**（例如 `/plan`、`/build` 依赖 M3，`/sessions` 的真实会话依赖 Remote）**不隐藏**：显示为不可用并给出原因，
   执行时提示原因而不是静默无反应。原因文字集中在注册表里，后续任务接通能力时只改可用性。
4. **slash 补全**：输入区以 `/` 开头时弹出补全（模糊匹配命令名与标题，照 autocomplete.tsx 的行为：↑↓ 选、Tab / Enter 接受、
   Esc 关闭、继续输入过滤）。补全弹层做成**通用组件**（触发字符 + 候选提供者），下一件任务会给 `@` 接文件候选。
   带参数的命令（`/effort high`、`/permission full`）接受参数；参数非法给出明确错误。
5. **命令面板**（`Ctrl+P`）：按分类分组、显示键位、不可用项置灰带原因；**帮助**（`/help`）：按上下文列出全部键位。
6. 已有功能不能回退：现有 `app.rs` 里所有键与命令迁到注册表后行为一致（含 Windows 的 F6 发送 / Enter 换行，
   见 `docs/CORE_HEADLESS_AUDIT_2026-10-14.md` §4）。
7. 代码组织：注册表与键位解析放新文件（如 `commands.rs`、`keymap.rs`），补全弹层放 `autocomplete.rs`；`app.rs` 只保留分派。
8. 测试：键位字符串解析（含非法值）；上下文优先级（对话框内的 Ctrl+F 不触发会话级动作）；leader 超时；用户覆盖与回退；
   slash 补全的过滤 / 接受 / 关闭；带参数命令；不可用命令执行时给原因；注册表里每个 action 都能从面板触发
   （遍历测试，防止漏接）；`TestBackend` 渲染面板、补全弹层、which-key、帮助。

## 验收

```
cargo test -p arc-deck-tui --offline
cargo clippy -p arc-deck-tui --all-targets --offline --no-deps -- -D warnings
cargo test -p arc-deck-cli --features test-runtime --offline
```
