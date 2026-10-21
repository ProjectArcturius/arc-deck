# schemas —— 契约示例

**单一事实来源是 Rust 类型**，不是这里的手写 schema。

这里放的是**示例**，不是另一份会漂移的规格副本：

| 文件 | 对应类型 |
|---|---|
| `task.example.json` | `arc_deck_core::task::TaskSpec` |
| `config.example.json` | `arc_deck_core::config::WorkerConfig` |
| `result.example.json` | `arc_deck_core::result::TaskResult` |
| `event.example.jsonl` | `arc_deck_core::event::EventEnvelope` |

1.0 增加可选的 `TaskResult.runtime_exit_code`：只有采集到运行时自然退出码时才出现；
主动收进程树不填，避免把客户端终止信号冒充运行时自己的退出结论。

`crates/arc-deck-core/tests/schema_contract.rs` 会把这些示例解析成对应类型再重新
序列化，要求两者语义相等，并逐个钉住关键字段名。**改字段名、加必填字段、改枚举
取值都会让那个测试失败**，失败信息直接指出是哪一个字段。

## 为什么不生成 JSON Schema

多一份 JSON Schema 就多一处需要同步的副本，而它最容易出的问题恰恰是
「类型改了、schema 没改」—— 也就是这里想防的那件事。等真的出现外部消费者
（例如 GUI 或第三方脚本）需要机器可校验的 schema 时，再从 Rust 类型生成，
而不是手写维护。

## 与 DSH 线上形状的区别

这里的字段一律 `snake_case`（`task_id`、`exit_code`、`total_tokens`），是
**Saiku 自己的**契约。DSH 的 JSON-RPC 与通知用 `camelCase`（`sessionId`、
`promptTokens`），那层映射在 `arc-deck-dsh` 里完成，不要混用。
