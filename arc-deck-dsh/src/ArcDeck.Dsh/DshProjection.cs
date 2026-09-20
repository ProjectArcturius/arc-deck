using System.Text.Json.Nodes;
using ArcDeck.Dsh.View;

namespace ArcDeck.Dsh;

/// <summary>
/// 把一个会话的事件流折叠成界面能画的视图。
///
/// ## 为什么要有「权威文本」这个概念
///
/// 助手的正文在事件流里出现**两次**，而且形状不同：
///
///   · <c>assistant/chunk</c> 的 <c>text-delta</c> —— 逐 token 的增量，用来做流式观感
///   · <c>assistant/message</c> 的 <c>message.content[]</c> —— 该步的**权威全文**
///
/// 只认前者，会话重载后正文取决于「增量有没有全部落盘」；只认后者，就完全没有
/// 流式。两条都用而不区分，界面会出现**正文写两遍**。所以这里显式记一个
/// <see cref="EntryState.Authoritative"/>：权威文本一到，整块被替换，后续增量不再追加。
///
/// ## 为什么保留原文
///
/// 上游的事件词汇比本客户端认识的宽（reasoning、subagent、compaction、checkpoint……）。
/// 认不出来的**不能丢** —— 丢掉之后「这里发生过一件事」这个信息就没了，
/// 而它往往正是解释「为什么界面看起来卡住了」的那条。
/// 所以认不出的落到 <see cref="DshBlockKind.Unknown"/>，原文照收（但有上限）。
/// </summary>
public sealed class DshProjection
{
    /// <summary>认不出来的事件保留多少条原文。</summary>
    private const int MaxRecordedEvents = 200;

    private readonly DshSessionView _view;
    private readonly Dictionary<string, EntryState> _entries = new();
    private readonly Dictionary<string, DshBlock> _toolsByCallId = new(StringComparer.Ordinal);

    /// <summary>
    /// 内核内部簿记事件的原文（顺序即到达顺序）。
    ///
    /// 界面上不画，但**不能扔** —— 它们是「这里发生过一件事」的唯一痕迹，
    /// 而「为什么界面看起来卡住了」的答案常常就在里面。
    /// </summary>
    public List<DshBlock> Recorded { get; } = new();

    /// <summary>出现过的所有事件类型（含认得的）。排查「少了哪一类事件」时用它。</summary>
    public HashSet<string> RecordedTypes { get; } = new(StringComparer.Ordinal);

    private int _unknownCount;
    private int _currentTurn;

    public DshProjection(DshSessionView view) => _view = view;

    public DshSessionView View => _view;

    /// <summary>投影后发生了什么变化 —— 界面据此决定「更新哪一块」，而不是整页重画。</summary>
    [Flags]
    public enum Change
    {
        None = 0,
        Structure = 1,   // 新增/删除了消息
        Text = 2,        // 已有消息的正文变了（流式）
        Status = 4,      // 运行状态或标题变了
    }

    private sealed class EntryState
    {
        public DshMessageView Message { get; init; } = null!;
        public bool Authoritative { get; set; }
        public bool IsToolCard { get; init; }
    }

    public Change Apply(DshSessionEvent evt)
    {
        var change = Change.None;
        _view.UpdatedAt = evt.Time;

        switch (evt.Type)
        {
            case DshEventType.TurnStart:
                _currentTurn++;
                _view.TurnCount = Math.Max(_view.TurnCount, _currentTurn);
                break;

            case DshEventType.UserMessage:
                change |= AppendUserMessage(evt);
                break;

            case DshEventType.AssistantChunk:
                change |= AppendTextDelta(evt);
                break;

            case DshEventType.AssistantMessage:
                change |= ApplyAssistantMessage(evt);
                break;

            case DshEventType.ToolResult:
                change |= ApplyToolResult(evt);
                break;

            case DshEventType.SessionTitle:
                change |= ApplyTitle(evt);
                break;

            case DshEventType.RequestHeader:
                change |= ApplyRequestHeader(evt);
                break;

            case DshEventType.Error:
                change |= AppendError(evt);
                break;

            // 下面这些是**边界**事件：它们改变的是「进行中」的语义，
            // 但界面此刻不需要新块（轮次计数在 turn/start 里已经记过）。
            // 显式列出来，而不是丢到 default —— 这样新增一种边界事件时，
            // 读代码的人能看出它是被判断过、而不是被漏掉的。
            case DshEventType.TurnEnd:
            case DshEventType.StepStart:
            case DshEventType.StepEnd:
                break;

            default:
                change |= AppendUnknown(evt);
                break;
        }

        return change;
    }

    // ── 自己的消息 ───────────────────────────────────────────────────────

    private Change AppendUserMessage(DshSessionEvent evt)
    {
        if (evt.Data is not JsonObject data) return Change.None;

        // **只有真人说的才进界面。**
        //
        // 内核会把系统提示词、工作区指令、技能清单都作为 user/message 写进会话日志
        // —— 一次 `session/prompt` 在实测里因此产生了 4 条 user/message。
        // 全部画出来会让界面被几百行英文系统提示词淹没。
        var sourceKind = data["source"]?["kind"]?.GetValue<string>();
        if (!string.Equals(sourceKind, "user", StringComparison.Ordinal)) return Change.None;

        var text = JoinTextBlocks(data["content"]);
        if (string.IsNullOrWhiteSpace(text)) return Change.None;

        var message = new DshMessageView { IsMine = true, Meta = evt.Time.ToString("HH:mm") };
        message.Blocks.Add(DshBlock.Paragraph(text));
        _view.Messages.Add(message);
        return Change.Structure;
    }

    // ── 助手正文 ─────────────────────────────────────────────────────────

    private Change AppendTextDelta(DshSessionEvent evt)
    {
        if (evt.Data is not JsonObject data) return Change.None;

        var chunk = data["chunk"];
        if (chunk is not JsonObject c) return Change.None;

        var kind = c["type"]?.GetValue<string>();

        // finish 块是**模型失败的唯一出口**：请求没成、余额不足、鉴权失败之类
        // 都不会抛到协议层，只会在这里带一个 reason.failure。第一次端到端跑
        // 时正是因为不读它，界面表现为「发出去以后什么都没有」，无从排查。
        if (string.Equals(kind, DshChunkType.Finish, StringComparison.Ordinal))
        {
            var failure = c["reason"]?["failure"]?["message"]?.GetValue<string>();
            if (string.IsNullOrWhiteSpace(failure)) return Change.None;

            var message = new DshMessageView { IsMine = false, Meta = $"失败 · {evt.Time:HH:mm}" };
            var block = DshBlock.Paragraph(failure!);
            block.Kind = DshBlockKind.Unknown;
            block.SourceType = "assistant/chunk:finish";
            message.Blocks.Add(block);
            _view.Messages.Add(message);
            return Change.Structure;
        }

        if (!string.Equals(kind, DshChunkType.TextDelta, StringComparison.Ordinal)) return Change.None;

        var delta = c["text"]?.GetValue<string>();
        if (string.IsNullOrEmpty(delta)) return Change.None;

        var key = GroupKey(evt, c);

        if (!_entries.TryGetValue(key, out var state))
        {
            var message = new DshMessageView { IsMine = false, GroupKey = key };
            message.Blocks.Add(DshBlock.Paragraph(string.Empty));
            state = new EntryState { Message = message };
            _entries[key] = state;
            _view.Messages.Add(message);
        }

        // 权威全文已经到了 → 增量是旧的重复投递，丢弃（这正是「正文写两遍」的根因）。
        if (state.Authoritative) return Change.None;

        var paragraph = state.Message.Blocks[0];
        paragraph.Text += delta;
        return Change.Text;
    }

    private Change ApplyAssistantMessage(DshSessionEvent evt)
    {
        if (evt.Data is not JsonObject data) return Change.None;

        var turn = data["turn"]?.GetValue<int>() ?? 0;
        var step = data["step"]?.GetValue<int>() ?? 0;
        var key = $"a:{turn}:{step}";

        if (data["message"] is not JsonObject message) return Change.None;

        if (!_entries.TryGetValue(key, out var state))
        {
            // 流式阶段建的条目挂在 `a:{turn}:{step}:{index}` 上（index 要区分
            // 正文块与思考块），而权威全文是**按步**给的 —— 键不同，所以这里
            // 得先按步找一遍。找不到就会多出一条重复的助手消息：
            // 实测第一次跑通时，界面上「收到」出现了两次，就是这么来的。
            state = FindStreamedEntry(turn, step);

            if (state is null)
            {
                var view = new DshMessageView { IsMine = false, GroupKey = key };
                state = new EntryState { Message = view };
                _view.Messages.Add(view);
            }

            // 让后续的增量投递也认得这条 —— 权威全文之后还会来同一步的 finish 块。
            state.Message.GroupKey = key;
            _entries[key] = state;
        }

        // 权威全文覆盖同一步的流式增量。整块重建而不是逐段比对：
        // 「同一步里权威文本与增量不一致」这件事本身不该被容忍，
        // 以权威为准是唯一能保持不变量的做法。
        state.Message.Blocks.Clear();
        state.Message.Meta = Meta(data, evt);

        foreach (var block in message["content"] as JsonArray ?? new JsonArray())
        {
            if (block is not JsonObject b) continue;

            switch (b["type"]?.GetValue<string>())
            {
                case "text":
                    var text = b["text"]?.GetValue<string>();
                    if (!string.IsNullOrEmpty(text)) state.Message.Blocks.Add(DshBlock.Paragraph(text!));
                    break;

                case "tool-call":
                    var callId = b["id"]?.GetValue<string>();
                    var name = b["name"]?.GetValue<string>() ?? "tool";
                    var args = b["arguments"]?.ToString() ?? "{}";
                    var tool = DshBlockForTool(name, callId, args);
                    state.Message.Blocks.Add(tool);
                    if (callId is not null) _toolsByCallId[callId] = tool;
                    break;

                // reasoning 等将来再画。**显式记下来而不是当没有** ——
                // 界面上看不到思考过程，读代码的人应该能一眼看出这是没做，不是漏了。
                default:
                    break;
            }
        }

        state.Authoritative = true;
        return Change.Structure;
    }

    private static string Meta(JsonObject data, DshSessionEvent evt)
    {
        var turn = data["turn"]?.GetValue<int>() ?? 0;
        return $"第 {turn} 轮 · {evt.Time:HH:mm}";
    }

    /// <summary>
    /// 找这一步在流式阶段建出来的条目。
    ///
    /// 流式的键带 <c>index</c>（正文块与思考块要分开），权威全文不带。
    /// 所以按前缀扫一遍 —— 用 <c>_entries</c> 的键而不是消息列表，是因为
    /// 一个会话里同一步只可能有一个这样的条目。
    /// </summary>
    private EntryState? FindStreamedEntry(int turn, int step)
    {
        var prefix = $"a:{turn}:{step}:";
        foreach (var (key, state) in _entries)
        {
            if (key.StartsWith(prefix, StringComparison.Ordinal)) return state;
        }
        return null;
    }

    // ── 工具 ─────────────────────────────────────────────────────────────

    private static DshBlock DshBlockForTool(string name, string? callId, string arguments) => new()
    {
        Kind = DshBlockKind.Tool,
        ToolName = name,
        CallId = callId,
        ToolArguments = arguments,
        Text = string.Empty,
    };

    private Change ApplyToolResult(DshSessionEvent evt)
    {
        if (evt.Data is not JsonObject data) return Change.None;
        if (data["message"] is not JsonObject message) return Change.None;

        var content = message["content"] as JsonArray;
        var first = content?.FirstOrDefault() as JsonObject;
        var callId = first?["toolCallId"]?.GetValue<string>();
        if (callId is null) return Change.None;

        var text = JoinTextBlocks(first!["content"]);
        var isError = first?["isError"]?.GetValue<bool>() ?? false;

        if (!_toolsByCallId.TryGetValue(callId, out var tool))
        {
            // 结果先到、调用没到 —— 正常不该发生，但重放截断的日志时会出现。
            // 建一个只有结果的工具块，比丢掉结果好：丢掉会让「工具跑过了」这件事消失。
            tool = DshBlockForTool("工具", callId, "{}");
            var message2 = new DshMessageView { IsMine = false, Meta = evt.Time.ToString("HH:mm") };
            message2.Blocks.Add(tool);
            _view.Messages.Add(message2);
            _toolsByCallId[callId] = tool;
            return Change.Structure;
        }

        tool.Text = text;
        tool.IsError = isError;
        tool.HasResult = true;
        return Change.Text;
    }

    // ── 标题、模型、错误、未知 ───────────────────────────────────────────

    private Change ApplyTitle(DshSessionEvent evt)
    {
        var title = evt.Data?["title"]?.GetValue<string>();
        if (string.IsNullOrWhiteSpace(title)) return Change.None;
        if (string.Equals(_view.Title, title, StringComparison.Ordinal)) return Change.None;

        _view.Title = title!;
        return Change.Status;
    }

    private Change ApplyRequestHeader(DshSessionEvent evt)
    {
        var config = evt.Data?["header"]?["config"];
        if (config is not JsonObject c) return Change.None;

        var provider = c["provider"]?.GetValue<string>();
        var model = c["model"]?.GetValue<string>();
        if (provider is null || model is null) return Change.None;

        _view.ModelLabel = $"{provider} · {model}";
        return Change.Status;
    }

    private Change AppendError(DshSessionEvent evt)
    {
        var text = evt.Data?["message"]?.GetValue<string>()
                   ?? evt.Data?["failure"]?["message"]?.GetValue<string>();
        if (string.IsNullOrWhiteSpace(text)) return Change.None;

        var message = new DshMessageView { IsMine = false, Meta = $"错误 · {evt.Time:HH:mm}" };
        var block = DshBlock.Paragraph(text!);
        block.Kind = DshBlockKind.Unknown;
        block.SourceType = DshEventType.Error;
        message.Blocks.Add(block);
        _view.Messages.Add(message);
        return Change.Structure;
    }

    private Change AppendUnknown(DshSessionEvent evt)
    {
        // 只记账，**不进界面**。
        //
        // 实测教训：第一次跑通时，一次普通的 prompt 里
        // `permission/preset` / `sandbox/mode` / `approval/policy` /
        // `agent/inbox/spliced` / `request/context` 全都落到了「未知」这一类，
        // 于是对话里出现了七条原始 JSON —— 真正的内容被埋掉了。这些事件是
        // **内核的内部簿记**，它们不该出现在对话流里。
        //
        // 但也不能丢：它们是「某件事发生过」的唯一痕迹。所以进 `Recorded`，
        // 界面默认不画，排查时可以取出来看。
        RecordedTypes.Add(evt.Type);
        _unknownCount++;

        if (_unknownCount <= MaxRecordedEvents)
        {
            var block = DshBlock.Paragraph(evt.Raw.ToJsonString(Transport.JsonRpcLineTransport.JsonOptions));
            block.Kind = DshBlockKind.Unknown;
            block.SourceType = evt.Type;
            Recorded.Add(block);
        }

        return Change.None;
    }

    // ── 工具方法 ─────────────────────────────────────────────────────────

    private static string GroupKey(DshSessionEvent evt, JsonObject chunk)
    {
        if (evt.Data is not JsonObject data) return "a:?";
        var turn = data["turn"]?.GetValue<int>() ?? 0;
        var step = data["step"]?.GetValue<int>() ?? 0;
        var index = chunk["index"]?.GetValue<int>() ?? 0;

        // index 参与键：同一步里模型可以产出多块（正文块 + 思考块），
        // 不分开会让思考的增量混进正文。
        return $"a:{turn}:{step}:{index}";
    }

    /// <summary>把一个 content 数组里的 text 块拼起来 —— 内核有时把一段正文拆成多块。</summary>
    private static string JoinTextBlocks(JsonNode? content)
    {
        if (content is not JsonArray array) return string.Empty;

        var parts = new List<string>();
        foreach (var item in array)
        {
            if (item is not JsonObject block) continue;
            if (!string.Equals(block["type"]?.GetValue<string>(), "text", StringComparison.Ordinal)) continue;
            var text = block["text"]?.GetValue<string>();
            if (!string.IsNullOrEmpty(text)) parts.Add(text!);
        }
        return string.Join("\n", parts);
    }
}
