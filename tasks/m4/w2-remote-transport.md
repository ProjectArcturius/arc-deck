# 任务书 w2-remote-transport：Rust 版 DSH Remote 传输层（host / rpc / mux）

派工者：claude。仓库：arc-deck（Rust workspace）。你在独立 git worktree 里工作，别的工人同时在改 `crates/arc-deck-tui`。
**本任务只动 `crates/arc-deck-dsh`**（新模块）与 `docs/`（一份新文档）。

## 背景

TUI 要做真正的多轮会话（PRODUCT_PLAN §7.3 Session / 消息），这需要 DSH 的 **Remote 层**：`dsh --profile web` 起一个
本机 web 内核，HTTP `/api/<ns>/<method>` 做一元调用，WebSocket `/api/remote.mux` 做多路复用的流（follow 会话等）。
仓里已有一份 **C# 实现**（`arc-deck-dsh/src/ArcDeck.Dsh/Harness/`：`HarnessHost.cs`、`HarnessRpc.cs`、`HarnessMux.cs`、
`HarnessSessionClient.cs`），它对 **DSH 0.1.6** 实测过；现在钉住的是 **0.2.1-alpha.2**，协议**必须按新源码重新核对**，
不能照抄 C# 的结论。`AGENTS.md` 的 **P11–P16** 是 C# 那一版踩过的坑，**先读**，每一条都要在 Rust 版里有对应处理或说明为何不再适用。

DSH 源码（与钉住的运行时同一版本，只读）：`C:/Users/mc158/Documents/Projects/arc-deck/REFS/deepseek-harness/`
- `apps/cli/src/args.ts`、`profile-boot.ts`（`--profile web` 怎么起、`--patch` 怎么叠加）
- `packages/api/gateway/src/stream-protocol.ts`（mux 帧协议，`REMOTE_STREAM_MUX_PATH`）
- `packages/client/connection/src/`（`rpc.ts`、`rpc-host.ts`、`rpc-schema.ts`、`client/*.ts`：客户端怎么连、信封形状）
- web 内核启动时怎么打印带 token 的 URL、怎么换 cookie：自己在 `apps/web`、`packages/` 里找到确切出处。

Rust 侧现有可复用的：`crates/arc-deck-dsh/src/runtime.rs`（`RuntimeLayout::entry`、`render_patch`、`runtime_env`）、
`process.rs`（`RuntimeProcess`、`no_window`、`kill_tree_by_pid`）、`crates/arc-deck-cli/src/worker.rs` 的 `dsh_runtime_command`
（SDK 模式下怎么拼命令行、环境、密钥 —— web 模式照它的形状来）。依赖 `tokio-tungstenite`、`reqwest`（无 TLS）已加好。

## 要做的

1. 新模块 `crates/arc-deck-dsh/src/remote/`（`mod.rs` + 按需拆 `host.rs` / `rpc.rs` / `mux.rs`），在 `lib.rs` 里 `pub mod remote;`。
2. **host**：用 `node <bin.js> --profile web [--patch <文件>]` 起内核。
   - `DSH_HOME` 由调用方传入（**绝不能默认到 DSH Desktop 的真实 home**，参 P15：follow 会接管会话）；
     遥测关闭等环境照 `runtime_env`。
   - 从 stdout 识别带 token 的 URL，换出鉴权 cookie。**token URL 是凭据**：不进日志、不进错误消息、`Debug` 输出里打码。
   - Windows 上子进程不得弹窗（用 `CREATE_NO_WINDOW`，见 `process::no_window` 的注释与 commit b227176）；
     drop / 显式关闭时收掉整棵进程树。启动超时与「内核先退出」都要给出可读的错误（含 stderr 尾部，但不含 token）。
3. **rpc**：一元调用 `call(method, args) -> Result<serde_json::Value, RemoteError>`。信封形状、`args` 字段名必须与 Host 形参
   逐字一致这一约束（C# `HarnessRpc.cs` 的注释）要按 0.2.1 源码核对。`ok:false` 的错误结构化地带回
   （code / message / details），不吞掉。
4. **mux**：一条 WebSocket 上多条流。`open(method, args) -> RemoteStream`（能逐帧 `next().await`）、关闭单条流、
   连接断开时**所有**流都收到明确的结束原因（不能静默挂住）。每条流独立无界队列（P11 的「快照与第一次读之间的帧不丢」）。
   消息按**整条**解码 UTF-8（P12；tungstenite 已按消息组帧，但要写测试证明跨分片的汉字不坏）。
5. **文档** `docs/REMOTE_0.2.1.md`：0.2.1-alpha.2 的启动 / 换票 / 信封 / mux 帧协议，**每一条注明源码出处（文件:行）**，
   并列出与 C# / 0.1.6 结论的差异。这是下一个工人实现会话操作的依据。
6. **测试**
   - 离线单测：信封编解码、错误结构、mux 帧路由（两条流不串帧）、断线通知所有流、token 打码。
     需要服务端时写一个**进程内的假服务端**（tokio + tungstenite 就够），不要依赖真内核。
   - **live 测试**（`#[ignore]`，名字以 `remote_live_` 开头）：对**本机已安装的真实运行时**（用 `RuntimeLayout` 定位，
     见 `saiku runtime status`），在**临时目录**作 `DSH_HOME` 起 web 内核 → 换票 → 调一个只读方法（如会话列表，
     新 home 应为空）→ 经 mux 打开一条只读的流（找一个不需要已有会话的；没有的话说明并改为验证 mux 握手）→ 关闭并确认进程退出。
     **不发任何 prompt，不消耗模型配额**。要配置模型路由时可用假密钥（不会发请求）。
     在 Windows 上确认整个过程没有新开可见控制台（子进程用 no_window 即可，不必写探针）。

## 约束

- 注释写 why（尤其是「为什么不照 C# 那样做」）；标识符英文，注释可中文。不要重排你没改的代码。
- 不许再引入新依赖（`--offline`）。`remote/` 模块内 `cargo clippy -p arc-deck-dsh --no-deps` **不新增警告**
  （该 crate 别处有历史警告，不用管，也不要顺手改）。
- 不要提交（commit）；不要运行联网命令；不要碰 `REFS/`；不要改 DSH Desktop 的 home（`%APPDATA%\dsh-desktop`）。

## 验收（派工者原样复跑）

```
cargo test -p arc-deck-dsh --offline
cargo test -p arc-deck-dsh --offline -- --ignored remote_live_
```

最终回复写：做了什么；0.2.1 与 C#/0.1.6 的差异清单；每条 P11–P16 在 Rust 版里怎么处理；没做 / 拿不准的地方。
