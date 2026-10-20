//! 会话操作：Remote 层之上、产品视图之下的那一层。
//!
//! 每个方法对应 `packages/api/session-controller/src/index.ts` 里的一个 `@Remote` 方法；
//! `args` 的键是 Host 形参名（`list` 的形参叫 `_request`，其余叫 `request`，`modelCatalog` 没有参数）——
//! 写错不会报「参数名不对」，而是 `gateway/arguments-invalid` 或干脆读到空值。
//!
//! 返回的一律是 `arc_deck_core::conversation` 的中立类型：界面与 IPC 不看 DSH 的形状。

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use arc_deck_core::conversation::{
    CatalogEffort, CatalogFailure, CatalogModel, CatalogProvider, Conversation, ModelCatalog,
    ModelChoice, SearchHit, SessionSummary,
};
use serde_json::{json, Value};

use super::events::{self, Answer, EventSubscription};
use super::fold::{model_choice, ConversationFold};
use super::{RemoteError, RemoteFrame, RemoteMux, RemoteRpc, RemoteStream, WebKernel};

/// 打开会话时往回翻页的上限。真实会话最多实测 17 页（P13）；这只是保险，不是预期。
const MAX_PAGES: usize = 500;
/// 每页最多多少条消息。大一点少几次往返；内核默认 50。
const PAGE_MESSAGES: u64 = 200;
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

pub struct SessionClient {
    rpc: RemoteRpc,
    mux: RemoteMux,
}

impl SessionClient {
    pub async fn connect(kernel: &WebKernel) -> Result<Self, RemoteError> {
        Ok(Self::from_parts(RemoteRpc::for_kernel(kernel)?, RemoteMux::connect(kernel).await?))
    }

    pub fn from_parts(rpc: RemoteRpc, mux: RemoteMux) -> Self {
        Self { rpc, mux }
    }

    /// 会话列表。子 agent 会话也在里面（`parent_session_id` 有值），由界面决定要不要显示。
    pub async fn list(&self) -> Result<Vec<SessionSummary>, RemoteError> {
        let value = self.rpc.call("session/list", json!({ "_request": {} })).await?;
        Ok(parse_list(&value))
    }

    /// 新建一个会话，工作目录是 `cwd`（DSH 的写入沙箱以它为根）。
    pub async fn create(&self, cwd: &Path) -> Result<String, RemoteError> {
        let value = self
            .rpc
            .call("session/create", json!({ "request": { "cwd": cwd.display().to_string() } }))
            .await?;
        value["sessionId"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| RemoteError::Protocol(format!("session/create 没有返回 sessionId：{value}")))
    }

    /// 为会话选模型。作用于**下一轮**（§7.2：不是热切换当前这一轮）。
    pub async fn select_model(&self, session_id: &str, choice: &ModelChoice) -> Result<ModelChoice, RemoteError> {
        let mut request = json!({
            "sessionId": session_id,
            "provider": choice.provider,
            "model": choice.model,
        });
        if let Some(effort) = &choice.effort {
            request["reasoningEffort"] = json!(effort);
        }
        let value = self.rpc.call("session/selectModel", json!({ "request": request })).await?;
        model_choice(&value["selected"])
            .ok_or_else(|| RemoteError::Protocol(format!("session/selectModel 的返回里没有 selected：{value}")))
    }

    pub async fn model_catalog(&self) -> Result<ModelCatalog, RemoteError> {
        let value = self.rpc.call("session/modelCatalog", json!({})).await?;
        Ok(parse_catalog(&value))
    }

    /// 发一条消息。返回客户端铸的 `requestId` —— 内核把它记在那条用户消息的 `source.rpcId` 上。
    ///
    /// 回执只表示「进了收件箱」，不表示答完了：要看 follow 流里的 `turn/end`。
    pub async fn prompt(&self, session_id: &str, text: &str) -> Result<String, RemoteError> {
        let request_id = format!(
            "arcdeck-{}-{}-{}",
            std::process::id(),
            // requestId 会落入会话历史。内核重连会重建 SessionClient；旧的实例计数器
            // 从 1 重来，DSH 会把新消息去重并回 accepted=true，前端于是一直转扫描条。
            // 时间戳还区分后续进程复用同一个 Windows pid 的情况。
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos(),
            NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
        );
        self.rpc
            .call(
                "session/prompt",
                json!({ "request": {
                    "requestId": request_id,
                    "sessionId": session_id,
                    // `queue`：正在跑时排到下一轮；`steer` 会插进当前轮，那是另一个交互，首版不做。
                    "mode": "queue",
                    "content": [{ "type": "text", "text": text }],
                } }),
            )
            .await?;
        Ok(request_id)
    }

    /// 取消这个会话**当前这一轮**（不清空排队的消息）。SDK 那条路没有这个能力（§9.1）。
    pub async fn cancel(&self, session_id: &str) -> Result<(), RemoteError> {
        self.rpc
            .call("session/cancel", json!({ "request": { "sessionId": session_id } }))
            .await
            .map(|_| ())
    }

    /// 检索会话内容（`session/search`）。命中片段由内核给，命中词用 `\u{0}` 夹起来。
    pub async fn search(&self, query: &str) -> Result<Vec<SearchHit>, RemoteError> {
        let value = self.rpc.call("session/search", json!({ "request": { "query": query } })).await?;
        Ok(parse_search(&value))
    }

    /// 打开转发事件流。审批请求走这条流（见 `events.rs`）。
    pub async fn subscribe_events(&self) -> Result<EventSubscription, RemoteError> {
        EventSubscription::open(&self.mux).await
    }

    /// 回答一条转发事件。`client_id` 是订阅建立时 Host 给的世代 id。
    pub async fn answer_event(
        &self,
        client_id: &str,
        request_id: &str,
        answer: Answer,
    ) -> Result<(), RemoteError> {
        events::answer(&self.rpc, client_id, request_id, answer).await
    }

    /// 改名。返回内核规范化之后的标题。
    pub async fn rename(&self, session_id: &str, title: &str) -> Result<String, RemoteError> {
        let value = self
            .rpc
            .call("session/rename", json!({ "request": { "sessionId": session_id, "title": title } }))
            .await?;
        Ok(value["title"].as_str().unwrap_or(title).to_string())
    }

    /// 打开一个会话：follow（带进程内流式帧）→ 等到快照 → 往回翻到历史取全。
    ///
    /// **follow 会接管冷会话**（P15）：这正是我们要的（之后才能继续对话），但也意味着
    /// 这个内核绝不能指向 DSH Desktop 的 home —— 见 `launch.rs`。
    pub async fn open(&self, session_id: &str) -> Result<OpenSession, RemoteError> {
        self.open_at(address(session_id, None)).await
    }

    /// 按地址打开（子 agent 会话要 [`address`] 给的父地址）。
    pub async fn open_at(&self, address: Value) -> Result<OpenSession, RemoteError> {
        let mut stream = self
            .mux
            .open("session/follow", json!({ "request": { "address": address, "assistantStream": true } }))
            .await?;
        let mut fold = ConversationFold::new();
        // 第一帧必须是快照（P11）。之后的帧都是 live，留在流的队列里不会丢（mux 每流一个无界队列）。
        match stream.next().await {
            Some(RemoteFrame::Item(item)) if item["type"] == "snapshot" => fold.apply_snapshot(&item),
            Some(RemoteFrame::Item(item)) => {
                return Err(RemoteError::Protocol(format!(
                    "session/follow 的第一帧不是快照：{}",
                    item["type"]
                )))
            }
            Some(RemoteFrame::Failure(failure)) => return Err(RemoteError::Remote(failure)),
            Some(other) => return Err(RemoteError::Closed(format!("follow 在快照之前就结束了：{other:?}"))),
            None => return Err(RemoteError::Closed("follow 在快照之前就结束了".into())),
        }

        let mut pages = 0;
        while let Some((through, before)) = fold.page_cursor() {
            if pages >= MAX_PAGES {
                break;
            }
            pages += 1;
            let value = self
                .rpc
                .call(
                    "session/page",
                    json!({ "request": {
                        "address": address,
                        "throughSeq": through,
                        "beforeSeq": before,
                        "maxMessages": PAGE_MESSAGES,
                    } }),
                )
                .await?;
            let records = value["records"].as_array().cloned().unwrap_or_default();
            fold.apply_page(&records, value["hasMore"].as_bool().unwrap_or(false));
        }
        Ok(OpenSession { stream, fold })
    }
}

/// 一个打开着的会话：follow 流 + 折好的视图。
pub struct OpenSession {
    stream: RemoteStream,
    fold: ConversationFold,
}

/// [`OpenSession::next`] 的结果。
#[derive(Debug)]
pub enum SessionUpdate {
    /// 视图变了（读 [`OpenSession::view`]）。
    Changed,
    /// 这一帧不影响视图（内部簿记、工具清单……）。
    Unchanged,
    /// 流结束了，原因原样带出（连接断了 / 内核报错 / 被关掉）。之后不会再有更新。
    Ended(String),
}

impl OpenSession {
    pub fn view(&self) -> &Conversation {
        self.fold.view()
    }

    pub fn session_id(&self) -> &str {
        &self.fold.view().session_id
    }

    /// 等下一帧并折进视图。
    pub async fn next(&mut self) -> SessionUpdate {
        match self.stream.next().await {
            Some(RemoteFrame::Item(item)) => {
                if self.fold.apply_follow_item(&item) {
                    SessionUpdate::Changed
                } else {
                    SessionUpdate::Unchanged
                }
            }
            Some(RemoteFrame::End) => SessionUpdate::Ended("内核结束了这条 follow 流".into()),
            Some(RemoteFrame::Failure(failure)) => SessionUpdate::Ended(failure.to_string()),
            Some(RemoteFrame::Disconnected(reason)) => SessionUpdate::Ended(reason),
            None => SessionUpdate::Ended("follow 流已关闭".into()),
        }
    }

    /// 关掉 follow（发 `cancel`，内核那边的迭代器随之结束）。
    pub fn close(&mut self) {
        self.stream.close();
    }
}

fn parse_search(value: &Value) -> Vec<SearchHit> {
    value["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let session_id = item["sessionId"].as_str().filter(|id| !id.is_empty())?;
            Some(SearchHit {
                session_id: session_id.to_string(),
                // `session/search` 的条目只有 id 与片段；标题由调用方从会话列表里补。
                title: None,
                snippet: item["snippet"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// 会话地址。子 agent 会话**必须**用父地址，否则内核拒绝：
/// `subagent Sessions require their durable parent address`（P13，C# 那条路实测）。
/// 列表项里没有 mode 时退回普通地址 —— 那是内核没说，不猜。
pub fn address(session_id: &str, summary: Option<&SessionSummary>) -> Value {
    match summary {
        Some(SessionSummary { parent_session_id: Some(parent), subagent_mode: Some(mode), .. }) => json!({
            "kind": "subagent",
            "parentSessionId": parent,
            "childSessionId": session_id,
            "mode": mode,
        }),
        _ => json!({ "kind": "session", "sessionId": session_id }),
    }
}

fn parse_list(value: &Value) -> Vec<SessionSummary> {
    let mut out: Vec<SessionSummary> = value["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            // 子 agent 描述符在 `projections.values.subagent`（`{ mode, label?, seq }`），**不是**
            // 列表项的顶层字段 —— schema 的缩进骗过 C# 那条路一次（P13）。顶层只作回退。
            let subagent = match &item["projections"]["values"]["subagent"] {
                Value::Object(_) => &item["projections"]["values"]["subagent"],
                _ => &item["subagent"],
            };
            SessionSummary {
            session_id: item["sessionId"].as_str().unwrap_or_default().to_string(),
            // 标题在 projections 里（P13 的那条更正：内核给的列表**带标题**）。
            title: item["projections"]["values"]["title"].as_str().map(str::to_string),
            cwd: item["cwd"].as_str().map(str::to_string),
            updated_at: item["updatedAt"].as_u64().unwrap_or(0),
            running: item["running"].as_bool().unwrap_or(false),
            blank: item["blank"].as_bool().unwrap_or(false),
            parent_session_id: item["parentSessionId"].as_str().map(str::to_string),
            subagent_mode: subagent["mode"].as_str().map(str::to_string),
            subagent_label: subagent["label"].as_str().map(str::to_string),
        }})
        .filter(|s| !s.session_id.is_empty())
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    out
}

fn parse_catalog(value: &Value) -> ModelCatalog {
    let text = |v: &Value| v.as_str().map(str::to_string);
    ModelCatalog {
        default: model_choice(&value["default"]),
        providers: value["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|group| CatalogProvider {
                id: text(&group["id"]).unwrap_or_default(),
                name: text(&group["name"]).unwrap_or_default(),
                models: group["models"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|model| CatalogModel {
                        id: text(&model["id"]).unwrap_or_default(),
                        name: text(&model["name"]).unwrap_or_default(),
                        description: text(&model["description"]),
                        // 形状按 0.2.1 的目录：窗口在模型自己身上，强度档在 `reasoning` 下。
                        context_window: model["contextWindow"].as_u64(),
                        efforts: model["reasoning"]["efforts"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|effort| CatalogEffort {
                                id: text(&effort["id"]).unwrap_or_default(),
                                name: text(&effort["name"]).unwrap_or_default(),
                                description: text(&effort["description"]),
                            })
                            .collect(),
                        default_effort: text(&model["reasoning"]["defaultEffort"]),
                    })
                    .collect(),
            })
            .collect(),
        failures: value["failures"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|failure| CatalogFailure {
                id: text(&failure["id"]).unwrap_or_default(),
                name: text(&failure["name"]).unwrap_or_default(),
                message: text(&failure["message"]).unwrap_or_default(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rebuilding_a_client_never_reuses_a_durable_prompt_id() {
        use crate::remote::fake::{FakeMux, FakeRpc};
        use crate::remote::Secret;
        let cookie = Secret::new("test=v1.a.b");
        let mut rpc = FakeRpc::start();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let mux = FakeMux::start();
            let client = SessionClient::from_parts(
                RemoteRpc::new(&rpc.origin(), &cookie).unwrap(),
                RemoteMux::connect_to(&mux.origin(), mux.port(), &cookie).await.unwrap(),
            );
            ids.push(client.prompt("restored-session", "next turn").await.unwrap());
            let raw = rpc.request().await.unwrap();
            let body: Value = serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(body["payload"]["args"]["request"]["requestId"], *ids.last().unwrap());
        }
        // DSH 以 requestId 在历史里去重；重复 id 会回 accepted=true，却永远不开新轮。
        assert_ne!(ids[0], ids[1], "内核重连后的新 prompt 被当成旧请求，前端会一直等");
    }

    /// 一条真实的 WS + 一个真实的 HTTP 假内核：审批流开出来、请求送到、回答回去。
    ///
    /// 为什么两边都要真的：开流走 WS（`$events`），回答走一元 HTTP（`$events/result`），
    /// 任何一边写错参数名，另一端都会以「读不到 / 参数非法」的形式暴露出来。
    #[tokio::test]
    async fn an_approval_request_is_answered_through_the_event_stream() {
        use crate::remote::fake::{FakeMux, FakeRpc};
        use crate::remote::{events::Forwarded, Secret};

        let mut mux = FakeMux::start();
        let mut rpc = FakeRpc::start();
        let cookie = Secret::new("dsh-auth-test=v1.a.b");
        let mux_client = RemoteMux::connect_to(&mux.origin(), mux.port(), &cookie).await.unwrap();
        let rpc_client = RemoteRpc::new(&rpc.origin(), &cookie).unwrap();
        let client = SessionClient::from_parts(rpc_client, mux_client);

        let mut opening = Box::pin(client.subscribe_events());
        // 两个 future 要同时推进：`subscribe_events` 发出 open 之后等 ready，
        // 而 open 帧只有在内核那一侧被读到之后才会有 ready。
        let open = tokio::select! {
            frame = mux.recv() => frame.expect("内核没收到 open"),
            _ = &mut opening => panic!("ready 之前不该开成"),
        };
        assert_eq!(open["type"], "open");
        assert_eq!(open["endpoint"], "$events");
        assert_eq!(open["payload"], serde_json::json!({ "args": {} }));
        let stream_id = open["streamId"].as_str().unwrap().to_string();
        mux.send(serde_json::json!({
            "type": "item",
            "streamId": stream_id,
            "value": { "type": "ready", "clientId": "client-1", "host": { "home": "/h" } },
        }));
        let mut subscription = opening.await.expect("ready 之后应该开成");
        assert_eq!(subscription.client_id(), "client-1");

        mux.send(serde_json::json!({
            "type": "item",
            "streamId": stream_id,
            "value": {
                "type": "waterfall",
                "event": "approval/request",
                "eventId": "evt-9",
                "agentId": "agent-1",
                "request": { "toolName": "bash", "callId": "call-1", "reason": "hook says ask" },
            },
        }));
        let Forwarded::Approval(request) = subscription.next().await else {
            panic!("第一帧应该是审批");
        };
        assert_eq!(request.tool_name, "bash");
        assert_eq!(request.request_id, "evt-9");

        client
            .answer_event(subscription.client_id(), &request.request_id, events::Answer::AllowOnce)
            .await
            .expect("回答应该发出去");
        let raw = rpc.request().await.expect("假一元接口没收到回答");
        assert!(raw.contains("$events/result"), "{raw}");
        assert!(raw.contains("allowed-once"), "{raw}");
        assert!(raw.contains("client-1") && raw.contains("evt-9"), "{raw}");
    }

    #[test]
    fn search_keeps_the_highlight_markers_and_drops_entries_without_an_id() {
        // 片段里的 \u{0} 是内核夹命中词的约定，原样留着给界面分段着色。
        let value = json!({ "items": [
            { "sessionId": "s1", "snippet": "改\u{0}会话\u{0}列表" },
            { "snippet": "没有 id" },
        ], "hasMore": false });
        let hits = parse_search(&value);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "s1");
        assert!(hits[0].snippet.contains('\u{0}'));
    }

    #[test]
    fn the_list_reads_titles_from_projections_and_orders_by_activity() {
        let value = json!({ "items": [
            { "sessionId": "old", "updatedAt": 1, "running": false, "blank": false, "cwd": "C:\\ws",
              "projections": { "kind": "sequenced", "asOfSeq": 3, "values": { "title": "旧的" } } },
            { "sessionId": "new", "updatedAt": 9, "running": true, "blank": false,
              "projections": { "kind": "cached", "asOfSeq": 1, "values": {} } },
            { "sessionId": "child", "updatedAt": 5, "parentSessionId": "new", "origin": "subagent" },
        ] });
        let list = parse_list(&value);
        assert_eq!(list.iter().map(|s| s.session_id.as_str()).collect::<Vec<_>>(), ["new", "child", "old"]);
        assert_eq!(list[2].title.as_deref(), Some("旧的"));
        assert_eq!(list[0].title, None, "没有标题就是没有，不编");
        assert!(list[0].running);
        assert_eq!(list[1].parent_session_id.as_deref(), Some("new"));
        assert_eq!(list[1].subagent_mode, None, "列表项没说 mode 就是没有");
    }

    /// 子 agent 的 mode 在 `projections.values.subagent`（P13）；有了它才能配出父地址。
    #[test]
    fn subagent_sessions_get_the_parent_address() {
        let value = json!({ "items": [
            { "sessionId": "child", "updatedAt": 5, "parentSessionId": "p", "origin": "subagent",
              "projections": { "values": { "subagent": { "mode": "one-shot", "label": "查依赖", "seq": 3 } } } },
            { "sessionId": "p", "updatedAt": 4, "projections": { "values": { "subagent": null } } },
        ] });
        let list = parse_list(&value);
        assert_eq!(list[0].subagent_mode.as_deref(), Some("one-shot"));
        assert_eq!(list[0].subagent_label.as_deref(), Some("查依赖"));
        assert_eq!(
            address("child", Some(&list[0])),
            json!({ "kind": "subagent", "parentSessionId": "p", "childSessionId": "child", "mode": "one-shot" })
        );
        assert_eq!(address("p", Some(&list[1])), json!({ "kind": "session", "sessionId": "p" }));
        assert_eq!(address("x", None), json!({ "kind": "session", "sessionId": "x" }));
    }

    #[test]
    fn the_catalog_keeps_efforts_defaults_and_failures() {
        // 形状取自 2026-10-19 真实抓帧的 `session/modelCatalog`。
        let value = json!({
            "default": { "provider": "deepseek-official", "model": "deepseek-flash" },
            "routableProviders": ["deepseek-official", "opencode-go"],
            "groups": [
                { "id": "opencode-go", "name": "OpenCode Go", "models": [
                    { "id": "deepseek-v4.1-flash", "name": "DeepSeek V4.1 Flash", "reasoning": {
                        "defaultEffort": "high",
                        "efforts": [{ "id": "low", "name": "Low" }, { "id": "high", "name": "High", "description": "默认" }] } }
                ] }
            ],
            "failures": [{ "id": "x", "name": "X", "message": "401" }]
        });
        let catalog = parse_catalog(&value);
        assert_eq!(catalog.default.as_ref().unwrap().model, "deepseek-flash");
        let model = &catalog.providers[0].models[0];
        assert_eq!(model.efforts.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["low", "high"]);
        assert_eq!(model.default_effort.as_deref(), Some("high"));
        assert_eq!(catalog.failures[0].message, "401");
    }
}
