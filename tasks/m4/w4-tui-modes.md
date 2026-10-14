# 任务书 w4-tui-modes：Agent / 模式 / 授权级别 / 审批在 TUI 上接通

（先读 `C:/Users/mc158/Documents/Projects/arc-deck/tasks/m4/_common.md`，全部条款适用。）

## 前置

- M3：`docs/M3_GUARD.md`（映射表 + 实测负例）；授权级别规格 PRODUCT_PLAN §5.6；Agent / 模式 §5.3–5.4、§7.4 的 `/plan` `/build` 规则。
- Remote 会话（w3-session-core）与审批请求的数据 / 应答方法（w2-remote-session 第 3 条）。
- 参考 v1.19.0：`routes/session/permission.tsx`（审批界面）、`question.tsx`、`component/dialog-agent.tsx`、`ui/dialog-confirm.tsx`。

## 要做的

1. **Agent 与模式**：`/agents`（`<leader>a`）选择 Echo / Build / Plan；`Shift+Tab` 循环；`/plan` 与 `/build` 严格按 §7.4 的规则
   （Echo → echo+plan / echo+build；Build → plan+plan；Plan → build+build；未知 Agent 不静默回退）。
   切换作用于**下一轮**；运行中切换须先结束 / 取消当前活动并确认（§7.4 最后一段）。界面上清楚显示当前 Agent 与模式。
   这些切换必须落到执行层（M3 的映射），不是只换提示词 —— 在测试里断言下一轮的会话配置确实带上了对应限制。
2. **授权级别**：`/permission readonly|workspace|full|confirm`；只能在启动授权的上限之内选（§5.6）；超出上限时说明原因。
3. **confirm 审批**：DSH 发来审批请求时，弹出审批界面（照 permission.tsx：显示工具、参数摘要 / diff、风险提示），
   选项按 Remote 支持的应答（允许一次 / 拒绝 / 其余按源码），键位进注册表。审批未应答时**界面明确在等你**，
   不被折叠、不被流式正文冲走；Esc 不等于允许。
4. 命令注册表里 `/plan` `/build` `/agents` `/permission` 的可用性改为可用（或按上限给原因）。
5. 测试：`/plan` `/build` 的六种转换；运行中切换的确认流；授权上限；审批弹出 / 允许 / 拒绝 / Esc 的行为与发回 Remote 的应答
   （假服务端断言）；审批等待时 UI 的可见性（`TestBackend`）。

## 验收

```
cargo test -p arc-deck-tui --offline
cargo clippy -p arc-deck-tui --all-targets --offline --no-deps -- -D warnings
cargo test -p arc-deck-cli --features test-runtime --offline
```
