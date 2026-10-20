//! 失败原因的**可判定分类**（PRODUCT_PLAN §6.5.5）。
//!
//! 调度者要的回答是「要不要再派一次」，不是一段英文。模型侧的错误原文只从一条
//! 通道出来（`finish.reason.failure`，参 AGENTS.md P3），落到这里被归成一个稳定的
//! code 与一个 `retryable` 判断 —— 省掉「回去翻日志」这一步，那正是最贵的一步。
//!
//! 两条纪律：
//!
//! 1. **原文永远保留。** 分类只加一层标签，不改写、不丢弃 provider 的原话 ——
//!    标签可能认错，原文不会。
//! 2. **认不出就保守。** 上游没有给错误分类，provider 的措辞也不保证稳定，所以
//!    这张表是尽力而为：命中不了就落回 `MODEL_FAILURE`，且 `retryable = false`
//!    （不认识的错不自动重派）。**分类错成「不可重派」只是多让人看一眼；分类错成
//!    「可重派」会白白烧一次配额** —— 两边的代价不对称，所以往保守那边倒。

/// 一次失败的可判定结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureClass {
    /// 稳定的原因码。落在 `status.json` 的 `last_error.code` 与 `result.json` 的 `blockers[].code`。
    pub code: &'static str,
    /// 调度者能不能靠「再派一次」解决。**是建议，不是自动动作**：Arc Deck 自己
    /// 不因为这一条去重试（§6.5.4 的重试只针对卡死与 DSML 泄漏）。
    pub retryable: bool,
}

const fn class(code: &'static str, retryable: bool) -> FailureClass {
    FailureClass { code, retryable }
}

/// 带上游结构化字段的分类。
///
/// `provider_code` 是 `turn/end` 的 `reason.error.code`，`status` 是 HTTP 状态码。
/// **优先用结构化字段**：真实会话里额度用尽是 `{code:"QUOTA", status:402}` ——
/// 比猜 `Insufficient Balance` 那串英文硬得多。认不出才退回子串匹配。
pub fn classify_with(status: Option<i64>, provider_code: Option<&str>, text: &str) -> FailureClass {
    if disk_full(text) || provider_code.is_some_and(disk_full) {
        return class("HOST_DISK_FULL", false);
    }
    if let Some(code) = provider_code {
        let c = code.to_ascii_lowercase();
        if c.contains("quota") || c.contains("balance") || c.contains("insufficient") {
            return class("MODEL_QUOTA_EXHAUSTED", false);
        }
        if c.contains("rate") || c.contains("overload") || c.contains("throttl") {
            return class("MODEL_RATE_LIMITED", true);
        }
        if c.contains("auth") || c.contains("unauthorized") || c.contains("forbidden") || c.contains("permission") {
            return class("MODEL_AUTH", false);
        }
        if c.contains("network") || c.contains("timeout") || c.contains("unavailable") {
            return class("MODEL_NETWORK", true);
        }
        if c.contains("context") {
            return class("MODEL_CONTEXT_LENGTH", false);
        }
        if c.contains("model") && (c.contains("not_found") || c.contains("invalid") || c.contains("unknown")) {
            return class("MODEL_ROUTE", false);
        }
    }
    if let Some(status) = status {
        match status {
            401 | 403 => return class("MODEL_AUTH", false),
            402 => return class("MODEL_QUOTA_EXHAUSTED", false),
            429 | 529 => return class("MODEL_RATE_LIMITED", true),
            // 5xx 是 provider 自己出问题，等一会儿再派有意义。
            500..=599 => return class("MODEL_SERVER_ERROR", true),
            _ => {}
        }
    }
    classify_model_failure(text)
}

/// 把模型侧的失败原文归成 `MODEL_*` 里的一类。
///
/// 判据全部是**大小写无关的子串**；状态码（`429`/`402`/`401`…）要求前后不是数字，
/// 免得把 `1429` 或 `4290` 认成状态码。顺序按「越具体越先」（额度先于限流：
/// 一条 `429 insufficient_quota` 是额度用尽，重派不会好）。
pub fn classify_model_failure(text: &str) -> FailureClass {
    // w2-remote-transport 的真失败是 ENOSPC，误归 MODEL_FAILURE 会让调度者重烧配额。
    if disk_full(text) {
        return class("HOST_DISK_FULL", false);
    }
    let lower = text.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));

    // 路由/模型名错。`supported API model names` 是实测原文（AGENTS.md P3 与
    // 离线夹具 Scenario::Failure 都用它）—— 改配置才好，重派无用。
    if has(&["supported api model names", "unknown model", "model not found", "invalid model"]) {
        return class("MODEL_ROUTE", false);
    }
    // 额度/余额。**排在限流前面**：额度用尽重派多少次都一样。
    if has_status(&lower, "402")
        || has(&["insufficient balance", "insufficient_quota", "quota exceeded", "no credit", "余额不足"])
    {
        return class("MODEL_QUOTA_EXHAUSTED", false);
    }
    if has_status(&lower, "401")
        || has_status(&lower, "403")
        || has(&["unauthorized", "invalid api key", "authentication_error", "invalid_api_key"])
    {
        return class("MODEL_AUTH", false);
    }
    if has_status(&lower, "429")
        || has_status(&lower, "529")
        || has(&["too many requests", "rate limit", "rate_limit", "overloaded"])
    {
        return class("MODEL_RATE_LIMITED", true);
    }
    if has(&[
        "econnrefused",
        "econnreset",
        "etimedout",
        "enotfound",
        "eai_again",
        "fetch failed",
        "socket hang up",
        "connect timeout",
        "proxy",
        "network",
    ]) {
        return class("MODEL_NETWORK", true);
    }
    if has(&["context length", "context_length", "too many tokens", "maximum context", "context window"]) {
        return class("MODEL_CONTEXT_LENGTH", false);
    }
    class("MODEL_FAILURE", false)
}

fn disk_full(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    ["enospc", "no space left on device", "disk full", "not enough space on the disk", "磁盘空间不足", "磁盘已满"]
        .iter().any(|needle| lower.contains(needle))
}

/// 一个原因码能不能靠重派解决。`status.json` 的 `last_error.retryable` 由它填。
///
/// 与 [`classify_model_failure`] 分开，是因为非模型侧的失败（卡死、验收、协议）
/// 也要回答同一个问题，而那些码在别处产生。
pub fn retryable(code: &str) -> bool {
    matches!(
        code,
        // 模型侧：限流与网络是典型的「等一会儿再派」。
        "MODEL_RATE_LIMITED"
            | "MODEL_NETWORK"
            // provider 自己出问题（5xx），与网络同一类：等一会儿再派。
            | "MODEL_SERVER_ERROR"
            // 模型偶发把工具调用当正文吐出来，重派一次通常就好。
            | "TOOL_CALL_AS_TEXT"
            // 运行时自己死了（OOM、崩溃）—— 不是配置错，重派有意义。
            | "RUNTIME_EXITED"
    )
}

/// 匹配 `429` 这类状态码。前后不允许还有数字，否则 `1429`、`4290` 会被误判。
fn has_status(text: &str, code: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0usize;
    while let Some(index) = text[from..].find(code) {
        let at = from + index;
        let before_ok = at == 0 || !bytes[at - 1].is_ascii_digit();
        let after = at + code.len();
        let after_ok = after >= bytes.len() || !bytes[after].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        from = at + code.len();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_host_disk_is_not_a_model_or_network_failure() {
        for text in ["ENOSPC: write", "No space left on device", "There is not enough space on the disk", "磁盘空间不足"] {
            assert_eq!(classify_model_failure(text), class("HOST_DISK_FULL", false));
            assert_eq!(classify_with(Some(503), Some("NETWORK"), text), class("HOST_DISK_FULL", false));
        }
        assert_eq!(classify_with(None, Some("ENOSPC"), "write failed"), class("HOST_DISK_FULL", false));
        assert!(!retryable("HOST_DISK_FULL"));
    }

    #[test]
    fn model_route_failure_is_not_retryable() {
        // 实测原文（夹具与 P3 用的是同一条）。
        let text = "The supported API model names are deepseek-flash, deepseek-v4-pro, but you passed deepseek-official.";
        let c = classify_model_failure(text);
        assert_eq!(c.code, "MODEL_ROUTE");
        assert!(!c.retryable);
    }

    #[test]
    fn quota_beats_rate_limit_when_both_appear() {
        // 一条 429 里写着额度用尽 —— 重派不会好，所以额度优先。
        let c = classify_model_failure("HTTP 429: insufficient_quota");
        assert_eq!(c.code, "MODEL_QUOTA_EXHAUSTED");
        assert!(!c.retryable);
    }

    #[test]
    fn rate_limit_and_network_are_retryable() {
        assert_eq!(classify_model_failure("429 Too Many Requests").code, "MODEL_RATE_LIMITED");
        assert!(classify_model_failure("429 Too Many Requests").retryable);
        assert_eq!(classify_model_failure("fetch failed: ECONNREFUSED 127.0.0.1:10809").code, "MODEL_NETWORK");
        assert!(classify_model_failure("socket hang up").retryable);
    }

    #[test]
    fn auth_and_quota_are_not_retryable() {
        assert!(!classify_model_failure("401 Unauthorized: invalid api key").retryable);
        assert!(!classify_model_failure("402 Insufficient Balance").retryable);
    }

    #[test]
    fn structured_fields_beat_substring_matching() {
        // 真实会话原文：`{message:"Insufficient Balance", code:"QUOTA", status:402}`。
        let c = classify_with(Some(402), Some("QUOTA"), "Insufficient Balance");
        assert_eq!(c.code, "MODEL_QUOTA_EXHAUSTED");
        assert!(!c.retryable);

        // 只看状态码也能判 —— 上游没给 code 时。
        assert_eq!(classify_with(Some(429), None, "").code, "MODEL_RATE_LIMITED");
        assert!(classify_with(Some(529), None, "").retryable);
        assert_eq!(classify_with(Some(402), None, "什么原文都没有").code, "MODEL_QUOTA_EXHAUSTED");
        assert_eq!(classify_with(Some(503), None, "").code, "MODEL_SERVER_ERROR");
        assert!(classify_with(Some(503), None, "").retryable);

        // 认不出的 code 与状态码要落回子串匹配，而不是硬编一个。
        assert_eq!(classify_with(Some(418), Some("TEAPOT"), "429 Too Many Requests").code, "MODEL_RATE_LIMITED");
        assert_eq!(classify_with(Some(418), Some("TEAPOT"), "没人见过的话").code, "MODEL_FAILURE");
    }

    #[test]
    fn unknown_text_falls_back_conservatively() {
        let c = classify_model_failure("something nobody has seen before");
        assert_eq!(c.code, "MODEL_FAILURE");
        // 认不出就不可重派：多看一眼的代价小于白烧一次配额。
        assert!(!c.retryable);
    }

    #[test]
    fn status_codes_need_a_boundary() {
        // `1429` 与 `4290` 不是状态码。
        assert_eq!(classify_model_failure("context used 1429 tokens").code, "MODEL_FAILURE");
        assert_eq!(classify_model_failure("id 4290 not found").code, "MODEL_FAILURE");
        // 但正常的形状要认。
        assert_eq!(classify_model_failure("status=429").code, "MODEL_RATE_LIMITED");
        assert_eq!(classify_model_failure("HTTP 429").code, "MODEL_RATE_LIMITED");
    }

    #[test]
    fn retryable_table_covers_the_non_model_codes() {
        assert!(retryable("MODEL_RATE_LIMITED"));
        assert!(retryable("TOOL_CALL_AS_TEXT"));
        assert!(!retryable("STALLED"));
        assert!(!retryable("CONFIG_INVALID"));
        assert!(!retryable("ACCEPTANCE_FAILED"));
        assert!(!retryable("MODEL_ROUTE"));
    }
}
