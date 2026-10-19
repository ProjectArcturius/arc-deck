# DSH 0.2.1-alpha.2 Remote 层：协议核对与 Rust 实现

日期：2026-10-20。对象：钉住的 `@deepseek-ai/dsh@0.2.1-alpha.2`（`REFS/deepseek-harness` 提交 `d7432673`）。
实现：`crates/arc-deck-dsh/src/remote/`。C# 那一版（`arc-deck-dsh/src/ArcDeck.Dsh/Harness/`）对 0.1.6 成立，**这里没有一条结论是从它抄来的** —— 形状相同的地方都在 0.2.1 源码里重新找到了出处。

## 一、证据层次

| 层 | 证明什么 | 怎么跑 |
|---|---|---|
| 离线单测（`remote::*`） | 我们自己的编解码、分发、折叠代码 | `cargo test -p arc-deck-dsh --offline` |
| live（不耗配额） | 我们对 0.2.1 的**理解**：启动、换票、一元调用、mux、会话操作 | `cargo test -p arc-deck-dsh --offline --lib -- --ignored remote_live_` |
| quota（耗一次极小配额） | 真实一轮：流式 → 权威全文 → turn/end；**夹具 `tests/fixtures/remote/one-turn.jsonl` 来自这里** | `cargo test -p arc-deck-dsh --offline --lib -- --ignored remote_quota_` |

离线夹具取自真实抓帧，不是按源码手写的形状（P9：论证不等于证据）。

## 二、启动与换票

| 事 | 0.2.1 的形状 | 出处 |
|---|---|---|
| 启动 | `node <bin.js> --profile web --patch <文件> --no-open --host 127.0.0.1 --port 0` | `apps/cli/src/args.ts:159-167` |
| 参数顺序 | launcher 遇到第一个不认识的 token 就把剩下全部交给 app，**`--patch` 必须在 `--no-open` 之前** | 同上 |
| 带票 URL | stdout 一行 `dsh web: http://127.0.0.1:<port>/?token=<32 字节 base64url>` | `packages/bundle/web-app/src/index.ts:280`、`client/connection/src/browser-auth.ts:14,55` |
| 换票 | `GET /?token=…` → **303** + `Set-Cookie: dsh-auth-<sha256>=v1.<body>.<sig>`；**不能跟随重定向**（cookie 挂在 303 那一跳） | `browser-auth.ts:250-292` |
| 凭据纪律 | 带票 URL 与 cookie 不进日志、错误消息与 `Debug`（`Secret`）；stderr 尾部进错误前先打码 | `remote/mod.rs`、`host.rs` |

## 三、一元调用与 mux

- **一元**：`POST /api/<ns>/<method>`，体是 `{type:"client-request", rpcId, method, payload:{args}}`；`payload` **有且只有** `args`，
  `args` 的键是 Host **形参名**（`gateway/src/index.ts:1138-1146`、`1478-1504`）。响应 `{ok:true,value}` / `{ok:false,error:{code,message,details}}`，失败结构化带回。
- **mux**：WS `/api/remote.mux`（`gateway/src/stream-protocol.ts:7`），帧 `open{streamId,endpoint,payload:{args}}` / `cancel{streamId}`；
  服务端 `item` / `end` / `error`。**0.2.1 的 `error` 本身就是终止帧**，后面不再跟 `end`。
- Host 每 2 秒发 Ping、约 6 秒没 Pong 就断（`gateway/src/stream-server.ts:40,105-124`）—— 所以读循环必须一直转。

## 四、会话方法（`packages/api/session-controller/src/index.ts`）

| 方法 | args | 返回 / 备注 |
|---|---|---|
| `session/list` | `{_request:{}}` | `items[]`：`sessionId`、`updatedAt`、`running`、`blank`、`cwd`、`parentSessionId`、`projections.values.title` |
| `session/create` | `{request:{cwd}}` | `{sessionId, agentPreset}` |
| `session/selectModel` | `{request:{sessionId, provider, model, reasoningEffort?}}` | `{selected}`；**作用于下一轮** |
| `session/modelCatalog` | `{}` | `{default, routableProviders, groups[{id,name,models[{id,name,reasoning:{efforts,defaultEffort}}]}], failures}` |
| `session/prompt` | `{request:{requestId, sessionId, mode:"queue", content:[{type:"text",text}]}}` | `{accepted:true}` —— **只表示进了收件箱** |
| `session/cancel` | `{request:{sessionId}}` | 取消当前轮，不清空排队消息 |
| `session/rename` | `{request:{sessionId, title}}` | `{title, seq}` |
| `session/follow`（流） | `{request:{address, assistantStream:true}}` | 第一帧快照，之后 `event` 与 `assistant-stream` |
| `session/page` | `{request:{address, throughSeq, beforeSeq, maxMessages}}` | `{records, hasMore}` |
| `session/control`（流） | `{}` | 全局基线 + projection 更新（标题、用量的实时值）—— **Rust 侧尚未使用** |

**模型默认值不是用户配置的那条**：实测 `modelCatalog.default` 是 DSH 出厂的 `deepseek-official/deepseek-flash`，
所以 `SessionClient` 的调用方建完会话要 `selectModel` 一次（TUI 桥接做了）。

## 五、事件（`packages/core/session/src/types.ts` 的 `SessionEventMap`）与折叠规则

真实一轮（2026-10-19 抓帧）的顺序：

```
快照 records: permission/preset · sandbox/mode · approval/policy · model/selection · agent/inbox/spliced · turn/start · agent/inbox/spliced
live:  step/start → system/message → user/message(kind=user) → user/message(runtime-context / skill-catalog / time-context)
       → request/header → request/context → stream:start → session/title(fallback) → session-title 请求
       → stream:chunk(block-start reasoning / block-end / block-start text / text-delta / block-end / usage / finish)
       → assistant/message(seq 17) → stream:end{committed, seq 17} → step/end → turn/end{completed} → session/title(模型生成)
```

| 规则 | 理由（对应 AGENTS 条目） |
|---|---|
| 0.2.1 **没有落盘的 `assistant/delta`**；流式只在 `assistant-stream` 帧里，需 `assistantStream:true` | 0.1.6 的打包行 / `assistant/chunk` 规则不适用（P7 只对旧组合成立） |
| 权威 `assistant/message` 到达时**整块替换**同一 step 的流式块；之后到的 chunk 丢弃 | P4；实测 `end` 帧晚于权威事件 |
| 失败 / 放弃的 attempt（`assistant/attempt` 或 `end` 非 committed）删掉它的流式块，只计重试次数 | 重试成功的轮次不挂失败块 |
| 模型失败 = `turn/end.reason.kind == "error"` 的 `error.message` / `code` | P3 |
| 只有 `source.kind == "user"` 的 `user/message` 是人说的话 | P6；同一轮里内核还写了三条别的 |
| 一轮一条助手消息，思考 / 正文 / 工具按发生顺序成块 | P8 |
| 空思考块（`text:""`）不画 | 实测存在 |
| 往回翻：`throughSeq` = 快照游标，`beforeSeq` = 已知最早 seq；空页或 `hasMore:false` 即停 | `history.ts` 的 `paginate`；P13 的闸 |
| 翻页后对「与对话有关的」事件副本整体重放；系统提示词、工具清单等不进副本 | 往回翻得到更早的事件，增量接不上 |

## 六、与 C# / 0.1.6 结论的差异

| 主题 | 0.1.6（C#） | 0.2.1（本实现） |
|---|---|---|
| 流式正文 | 落盘的 `assistant/chunk` + 打包行（P7） | 进程内 `assistant-stream` 帧，可选 |
| 翻页 | `throughSeq` 往回走、`-1` 是空页（P13） | `throughSeq` 固定为快照游标，`beforeSeq` 往回走 |
| 模型默认 | —（C# 由调用方传路由） | 内核出厂默认，需按会话选择 |

只列两边都有证据的差异。mux 的 `error` 是否终止、`--patch` 的位置在 0.1.6 上**没有核对过**，不写成「差异」。

## 七、AGENTS P11–P16 在 Rust 版里的处理

| 条目 | 处理 |
|---|---|
| P11 快照即 opening | `SessionClient::open` 只等第一帧快照；之后的帧留在流队列（mux 每流无界队列） |
| P12 分块撕开汉字 | tungstenite 按整条消息组帧；`a_multibyte_character_split_across_websocket_fragments_survives` |
| P13 一页 / 子 agent 地址 | 翻页见上；子 agent 父地址的形状在 `types.ts` 的 `SessionAddress`，**Rust 侧尚未接**（列表里有 `parent_session_id`，打开子会话未做） |
| P14 / P15 两个写者 | 交互内核的 `DSH_HOME` 独立且加独占锁（`launch::HomeLock`）；live 测试一律临时目录 |
| P16 实时帧包装 / 当轮断言 | 实时与快照同形 `{type:'event', event}`，同一个 `apply_follow_item`；quota 测试只看发消息后新增的部分 |

## 八、离线测试曾经挂住的原因（已修）

并行跑全套时 mux 的两条测试偶尔挂 60 秒以上、单跑不复现。根因两处：
1. **假服务端**：Windows 上从非阻塞 listener `accept` 出来的 socket **继承非阻塞**（Linux 不继承），握手读到 WouldBlock 就退出，而 `control` 里的克隆句柄让 TCP 连接继续开着。
2. **产品代码**：客户端握手读**没有超时**，`connect_to` 超时之后又去 `join` 那条永远阻塞在握手里的线程 —— 真内核接了 TCP 却不回 HTTP 时，产品同样会挂死。现在握手读写以 `HANDSHAKE_TIMEOUT` 为界。

## 九、没做的

- 审批（`confirm`）：数据与应答方法未接；交互内核遇到 `confirm` 拒绝启动，不悄悄当成 `workspace`。
- `session/control` 的实时 projection（用量、上下文压力的实时值）；子 agent 会话的打开；带附件的一元响应（multipart，明确报不支持）；`steer` 模式。
