using System.Text.Json.Nodes;

namespace ArcDeck.Dsh;

/// <summary>会话里的一帧事件的类型标签。取值都是上游的原文，没有加工。</summary>
public static class DshEventType
{
    public const string TurnStart = "turn/start";
    public const string TurnEnd = "turn/end";
    public const string StepStart = "step/start";
    public const string StepEnd = "step/end";
    public const string UserMessage = "user/message";
    public const string AssistantMessage = "assistant/message";
    public const string AssistantChunk = "assistant/chunk";
    public const string ToolCall = "tool/call";
    public const string ToolResult = "tool/result";
    public const string SessionTitle = "session/title";
    public const string RequestHeader = "request/header";
    public const string RequestContext = "request/context";
    public const string Error = "error";

    // ── 打包行：日志的**压缩编码**，不是事件 ─────────────────────────────
    //
    // 上游把连续的同类增量块压成一行（实测：一份 5127 行的日志里，1552 帧
    // `assistant/chunk` 与 1552 行 `tool-call-chunks` 并存，而真正的
    // `text-delta` 只有 1 帧 —— 绝大多数正文增量都在 `text-chunks` 里）。
    // 打包行没有 `seq`，形状也与事件不同。
    //
    // 读日志时**必须显式跳过**：展开它们会把内容写两遍，而靠「不认识就忽略」
    // 那条兜底规则会让后人以为这是漏掉的类型，而不是刻意跳过的。
    public const string PackedTextChunks = "text-chunks";
    public const string PackedReasoningChunks = "reasoning-chunks";
    public const string PackedToolCallChunks = "tool-call-chunks";

    /// <summary>日志首行那一帧会话头（不是事件，没有 seq）。</summary>
    public const string SessionHeader = "session";
}

/// <summary>
/// 一帧归一化后的事件。
///
/// **原始 JSON 一定保留**（<see cref="Raw"/>）：本客户端的翻译表必然会漏掉
/// 上游新增的块类型，而丢掉原文等于把「以后能补上」变成「当时的信息已经没了」。
/// 界面上暂时不认识的类型先落到原文，将来加翻译时不需要重新跑一遍会话。
/// </summary>
public sealed record DshSessionEvent(
    string SessionId,
    string Type,
    long Sequence,
    DateTimeOffset Time,
    JsonNode? Data,
    JsonObject Raw)
{
    public static DshSessionEvent? FromNotification(string sessionId, JsonNode? payload)
    {
        if (payload is not JsonObject envelope) return null;

        var type = envelope.TryGetPropertyValue("type", out var t) ? t?.GetValue<string>() : null;
        if (string.IsNullOrEmpty(type)) return null;

        var seq = envelope.TryGetPropertyValue("seq", out var s) && s is not null
            ? s.GetValue<long>() : -1L;

        var timeMs = envelope.TryGetPropertyValue("time", out var tm) && tm is not null
            ? tm.GetValue<long>() : 0L;

        return new DshSessionEvent(
            SessionId: sessionId,
            Type: type!,
            Sequence: seq,
            Time: timeMs > 0 ? DateTimeOffset.FromUnixTimeMilliseconds(timeMs).ToLocalTime() : DateTimeOffset.Now,
            Data: envelope.TryGetPropertyValue("data", out var d) ? d : null,
            Raw: envelope);
    }
}

/// <summary>
/// 助手文本的增量块。
///
/// 上游把助手的 token 流拆成 <c>text-delta</c> / <c>reasoning-delta</c> /
/// <c>tool-call-delta</c> 三种；界面只消费第一种（正文），其余两种暂时只记账。
/// </summary>
public static class DshChunkType
{
    public const string TextDelta = "text-delta";
    public const string ReasoningDelta = "reasoning-delta";
    public const string ToolCallDelta = "tool-call-delta";
    public const string Finish = "finish";
}
