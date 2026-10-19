//! 会话操作：Remote 层之上、产品视图之下的那一层。
//!
//! 每个方法对应 `packages/api/session-controller/src/index.ts` 里的一个 `@Remote` 方法；
//! `args` 的键是 Host 形参名（`list` 的形参叫 `_request`，其余叫 `request`，`modelCatalog` 没有参数）——
//! 写错不会报「参数名不对」，而是 `gateway/arguments-invalid` 或干脆读到空值。
//!
//! 返回的一律是 `arc_deck_core::conversation` 的中立类型：界面与 IPC 不看 DSH 的形状。

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_deck_core::conversation::{
    CatalogEffort, CatalogFailure, CatalogModel, CatalogProvider, Conversation, ModelCatalog,
    ModelChoice, SessionSummary,
};
use serde_json::{json, Value};

use super::fold::{model_choice, ConversationFold};
use super::{RemoteError, RemoteFrame, RemoteMux, RemoteRpc, RemoteStream, WebKernel};

/// 打开会话时往回翻页的上限。真实会话最多实测 17 页（P13）；这只是保险，不是预期。
const MAX_PAGES: usize = 500;
/// 每页最多多少条消息。大一点少几次往返；内核默认 50。
const PAGE_MESSAGES: u64 = 200;

pub struct SessionClient {
    rpc: RemoteRpc,
    mux: RemoteMux,
    next_request: AtomicU64,
}

impl SessionClient {
    pub async fn connect(kernel: &WebKernel) -> Result<Self, RemoteError> {
        Ok(Self::from_parts(RemoteRpc::for_kernel(kernel)?, RemoteMux::connect(kernel).await?))
    }

    pub fn from_parts(rpc: RemoteRpc, mux: RemoteMux) -> Self {
        Self { rpc, mux, next_request: AtomicU64::new(1) }
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
            "arcdeck-{}-{}",
            std::process::id(),
            self.next_request.fetch_add(1, Ordering::SeqCst)
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
        let address = json!({ "kind": "session", "sessionId": session_id });
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

fn parse_list(value: &Value) -> Vec<SessionSummary> {
    let mut out: Vec<SessionSummary> = value["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| SessionSummary {
            session_id: item["sessionId"].as_str().unwrap_or_default().to_string(),
            // 标题在 projections 里（P13 的那条更正：内核给的列表**带标题**）。
            title: item["projections"]["values"]["title"].as_str().map(str::to_string),
            cwd: item["cwd"].as_str().map(str::to_string),
            updated_at: item["updatedAt"].as_u64().unwrap_or(0),
            running: item["running"].as_bool().unwrap_or(false),
            blank: item["blank"].as_bool().unwrap_or(false),
            parent_session_id: item["parentSessionId"].as_str().map(str::to_string),
        })
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
