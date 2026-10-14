# 任务书 w3-session-core：TUI 的会话改走 Remote（多轮、流式、思考、取消）

（先读 `C:/Users/mc158/Documents/Projects/arc-deck/tasks/m4/_common.md`，全部条款适用。）

## 前置

- `crates/arc-deck-dsh/src/remote/` 与 `docs/REMOTE_0.2.1.md`（w2-remote-transport、w2-remote-session 的成果）。
- 现状：TUI 每次提交起一个独立的 `arcdeck run --prompt -`（M4a），没有多轮。`crates/arc-deck-cli/src/tui.rs` 是桥接。
- 规格：PRODUCT_PLAN §7.3 的 Session、输入框、消息三行；§7.1 的「窗口适应、操作逻辑、构图」。
- 参考 v1.19.0：`routes/session/index.tsx`（构图、消息、滚动、思考折叠）、`routes/session/footer.tsx`、`routes/home.tsx`、
  `util/transcript.ts`、`util/scroll.ts`。

## 要做的

1. **宿主**：`tui.rs` 在 TUI 启动时（或第一次需要时）起一个 Remote host，`DSH_HOME` 用 Arc Deck 自己的交互 home
   （`<数据目录>/arcdeck/dsh-home`，**不是** Desktop 的），模型路由与密钥照 headless 的做法注入。TUI 退出时收掉内核进程树。
   启动失败（运行时没装、密钥缺失）在界面上给出可操作的原因（例如「先运行 `arcdeck runtime install`」），不 panic、不空白。
2. **会话流**：Home 输入 → 新建会话并发送；之后同一会话里继续对话（多轮）。正文**流式**出现，权威全文到达后替换流式段落且
   **不重复**（P4）；思考块默认折叠、可展开；工具调用以现有工具摘要组件显示（完整工具视图是另一件任务）；模型失败以失败块显示原文（P3）。
3. **滚动**：流式更新时若用户已上滚，**不把用户拉回底部**、不丢锚点；`/latest` / Ctrl+End 回到最新（§7.3 Session 的验收）。
4. **取消**：Esc 分层语义不变；运行中 Esc → 取消当前轮（Remote 的会话取消），界面显示「已取消」且输入区恢复可用。
5. **模型 / effort / Agent**：对话框里的选择作用于**当前会话的下一轮**（Remote 的选择模型方法），界面说清「下一轮生效」。
6. **headless 运行历史保留**：M4a 的「运行」视图（看 `arcdeck` 工人任务的状态 / 事件）不删，改为一个独立入口（如 `/runs`），
   用来监看派出去的工人 —— 这对日常派工有用。会话与运行是两样东西，不要混在一个列表里。
7. 命令注册表（w3-commands 的成果）里与会话相关的命令改为可用：`/new`、`/cancel`、`/latest` 等；`/sessions` 在下一件任务接通。
8. 测试：用 w2 的假服务端（或在 TUI 层注入一个假的会话源）驱动 `App`：多轮、流式 → 权威替换不重复、上滚时流式不拉回、
   取消、失败块、思考折叠、启动失败的提示；`TestBackend` 在 80×24 / 120×30 / 40×12 渲染。live 测试（`#[ignore]`，
   `tui_live_` 前缀，临时 home，不发 prompt）：TUI 后端能起 host 并新建空会话。

## 验收

```
cargo test -p arc-deck-tui --offline
cargo clippy -p arc-deck-tui --all-targets --offline --no-deps -- -D warnings
cargo test -p arc-deck-cli --features test-runtime --offline
cargo test -p arc-deck-cli --features test-runtime --offline -- --ignored tui_live_
```
