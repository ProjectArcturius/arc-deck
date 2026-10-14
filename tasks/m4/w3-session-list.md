# 任务书 w3-session-list：会话列表、恢复、改名、历史翻页、真实模型目录

（先读 `C:/Users/mc158/Documents/Projects/arc-deck/tasks/m4/_common.md`，全部条款适用。）

## 前置

w3-session-core 已让 TUI 走 Remote。参考 v1.19.0：`component/dialog-session-list.tsx`、`dialog-session-rename.tsx`、
`dialog-session-delete-failed.tsx`、`routes/session/dialog-timeline.tsx`、`dialog-message.tsx`；PRODUCT_PLAN §7.2（模型目录与 DSH）。

## 要做的

1. **会话列表**（`/sessions`、`<leader>l`）：当前工作区的会话（按 cwd 过滤，可切换显示全部），按最近更新排序，可模糊搜索；
   显示标题、相对时间、是否在跑；子 agent 会话不混在顶层（可展开或不显示，写清选择）。Enter 打开并**恢复**：
   历史完整（往回翻页取全，或滚到顶时按需加载更早的页 —— 二选一，写清理由），之后可继续对话。
2. **改名**（列表里一个键 + 对话框）、**删除**（如果 Remote 有对应方法；没有就不做并在不可用原因里写明）。
3. **时间线 / 跳到某条消息**（照 dialog-timeline）：列出本会话的用户消息，选中后滚到那里。
4. **真实模型目录**：把 `tui.rs` 里从用户配置拼出来的临时目录换成 Remote 的模型目录（w2-remote-session 已提供）；
   `catalog::Model.efforts` 由此填上，effort 对话框因此对有档位的模型出现。目录拿不到时回退到临时目录并在对话框标题处注明来源。
5. 打开会话会 promote 冷会话（P15）—— 只在 Arc Deck 自己的交互 home 里做，确认不会碰 Desktop 的 home。
6. 测试：列表过滤 / 排序 / 搜索；打开后历史与线上一致（假服务端上分页场景）；改名后列表与会话头同步；时间线跳转；
   目录来源切换与回退；`TestBackend` 渲染列表与对话框。

## 验收

```
cargo test -p arc-deck-tui --offline
cargo clippy -p arc-deck-tui --all-targets --offline --no-deps -- -D warnings
cargo test -p arc-deck-cli --features test-runtime --offline
```
