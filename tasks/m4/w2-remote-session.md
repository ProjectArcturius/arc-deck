# 任务书 w2-remote-session：Remote 会话操作与模型目录（Rust）

派工者：claude（或替班 sol）。仓库：arc-deck。独立 worktree。**只动 `crates/arc-deck-dsh`（`remote/` 下）、
`crates/arc-deck-core`（如需新增中立数据类型）与 `docs/`。**

## 前置（先读）

- `docs/REMOTE_0.2.1.md` 与 `crates/arc-deck-dsh/src/remote/`：上一件任务（w2-remote-transport）做的 host / rpc / mux 与协议核对。
- `AGENTS.md` **P3–P16**（全部是会话层的坑：权威全文与增量、内部事件不是对话、`source.kind=="user"`、按 turn 归并、
  follow 的快照即 opening、快照只是一页要往回翻、子 agent 的父地址、follow 会 promote 会话、实时帧带包装、
  onEvent 只在投影变化时回调、当轮作用域的断言）。每一条在 Rust 版里都要有对应处理或说明为何不再适用。
- C# 参考：`arc-deck-dsh/src/ArcDeck.Dsh/Harness/HarnessSessionClient.cs`、`DshProjection.cs`（对 0.1.6 成立，要按 0.2.1 核对）。
- DSH 源码：`REFS/deepseek-harness/packages/api/session-controller/src/`（`list.ts`、`history.ts`、`control.ts`、`commands.ts`、
  `assistant-stream.ts`、`catalog.ts`、`model-selection-projection.ts`、`types.ts`、`remote-events.ts`）。
- Rust 现有投影：`crates/arc-deck-dsh/src/projection.rs`（SDK 路径用的）。**优先复用**；Remote 的事件与 SDK 有差异时扩展它，
  不要另写一份平行的投影（PRODUCT_PLAN：两条传输共用事件投影）。

## 要做的

1. `remote::session`（或按需拆文件）提供：会话列表（带标题、cwd、running、子 agent 描述）、新建会话、打开（follow：快照 + live）、
   往回翻页直到取全（P13 的游标规则与「无进展就停」闸）、发消息、取消当前轮、改名、选择模型 / effort、读模型目录。
   子 agent 用父地址（P13）。
2. 事件 → 产品视图：消息按 turn 归并；助手正文以权威全文替换流式段落（P4）；思考块、工具调用与结果配对、失败块
   （`finish.reason.failure` 是模型失败的唯一出口，P3）；内部簿记事件不进对话（P5/P6）。输出一个 TUI 能直接画的中立视图类型
   （放 `arc-deck-core` 或 `arc-deck-dsh`，**不依赖 ratatui**）。增量更新要有「哪条消息变了」的信息，供界面局部刷新。
3. 审批：把 DSH 的审批请求（approval 相关事件 / 方法，按源码找）暴露为「待应答的请求」+ 应答方法（允许一次 / 拒绝 / 按源码支持的选项）。
   本任务只做数据与方法，界面在 w4-tui-modes。
4. 模型目录：`catalog.ts` 的结果转成中立类型，**字段要能填满** `crates/arc-deck-tui/src/catalog.rs` 的 `Provider` / `Model`
   （含 `efforts`、`release_date`、免费与否、不可用原因 —— 源码里没有的字段就留空，不编）。
5. 测试
   - 离线：在 transport 那件任务的假服务端上加会话场景（快照 + live 帧、分页、子 agent、权威全文替换流式、内部事件被过滤、
     失败块、审批请求），断言视图。P9 的教训：**对账要覆盖实际走的那条路**（live 帧与快照两条都要测）。
   - live（`#[ignore]`，`remote_live_` 前缀，临时 `DSH_HOME`，不发 prompt）：列表、新建、打开空会话、改名、读模型目录（断言非空且含配置的路由）。
   - **配额测试**（`#[ignore]`，`remote_quota_` 前缀）：用 `opencode-go/deepseek-v4.1-flash` 发**一条极短**消息（如「只回复：收到」），
     断言 live 事件到 turn 结束、本轮新增一条助手消息且正文非空、无失败块（只看本轮新增，P16 第三条）。用户已允许少量 flash 测试。
6. 文档：在 `docs/REMOTE_0.2.1.md` 追加会话层的方法与事件对照（源码出处）。

## 约束

`_common.md` 的通用条款（不提交、不联网、不碰 REFS、注释写 why、不引入新依赖）同样适用；不要改 Desktop 的 home。
`remote/` 内不新增 clippy 警告。

## 验收

```
cargo test -p arc-deck-core -p arc-deck-dsh --offline
cargo test -p arc-deck-dsh --offline -- --ignored remote_live_
cargo test -p arc-deck-dsh --offline -- --ignored remote_quota_
```
