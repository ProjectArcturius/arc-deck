using System.Text.Json.Nodes;
using ArcDeck.Dsh.View;

namespace ArcDeck.Dsh;

/// <summary>
/// 把一个会话的事件流折叠成界面能画的视图。
///
/// ## 一条消息 = 一轮，不是一步
///
/// 内核的 <c>turn</c> 是「用户说一次话」，<c>step</c> 是这一轮里模型的一次调用。
/// 一次 agentic 的运行会有**几十到几百个 step**（实测本机一份会话：1 轮 238 步，
/// 每步两三个 <c>tool-call</c> 块）。
///
/// 早先按 step 建消息，结果是 238 个气泡、其中绝大多数只有一个工具卡 ——
/// 列表被拆得没法读。所以按 <c>turn</c> 归并：一轮一条助手消息，
/// 里面按顺序放正文段、工具卡、思考块。
///
/// ## 为什么要有「权威文本」这个概念
///
/// 正文在事件流里出现**两次**，形状不同：
///
///   · <c>assistant/chunk</c> 的 <c>text-delta</c> —— 逐 token 增量（只用于实时流式）
///   · <c>assistant/message</c> 的 <c>message.content[]</c> —— 该步的**权威全文**
///
/// 两条都用而不区分，界面会出现**正文写两遍**。所以：注入模式（实时）消费增量，
/// 重放模式（读日志）**只看权威全文**。
///
/// ## 为什么重放必须走同一条折叠路径
///
/// 日志里没有「消息」，只有事件 —— 哪个显示、哪个不显示完全取决于折叠规则。
/// 另写一份「读日志」的逻辑会立刻产生第二个真相源，而两者的差异只在特定会话上
/// 才显形（比如带工具调用的那几步）。所以重载复用本类，只是把
/// <see cref="DshProjection(DshSessionView, bool)"/> 的 replay 打开。
/// </summary>
public sealed class DshProjection
{
    /// <summary>认不出来的事件保留多少条原文。</summary>
    private const int MaxRecordedEvents = 200;

    private readonly bool _replay;
    private readonly DshSessionView _view;
    private readonly Dictionary<string, DshMessageView> _messagesById = new(StringComparer.Ordinal);
    private readonly Dictionary<string, DshBlock> _toolsByCallId = new(StringComparer.Ordinal);

    /// <summary>每一轮的助手消息 —— 键是 turn。见类注释里的「一条消息 = 一轮」。</summary>
    private readonly Dictionary<int, DshMessageView> _assistantByTurn = new();

    /// <summary>正在流式追加的那条消息（实时模式）。权威全文一到就置空。</summary>
    private DshMessageView? _streaming;
    private DshBlock? _streamingParagraph;
    private string? _streamingKey;

    private int _unknownCount;
    private int _currentTurn = -1;

    /// <param name="replay">
    /// true = 从落盘日志重放：**忽略流式增量**，只认权威全文与工具事件。
    /// 日志里那些增量要么被压缩成打包行、要么与权威全文重复，都不该再叠一遍。
    /// </param>
    public DshProjection(DshSessionView view, bool replay = false)
    {
        _view = view;
        _replay = replay;
    }

    public DshSessionView View => _view;

    /// <summary>
    /// 内核内部簿记事件的原文（顺序即到达顺序）。
    /// 界面上不画，但**不能扔** —— 它们是「这里发生过一件事」的唯一痕迹。
    /// </summary>
    public List<DshBlock> Recorded { get; } = new();

    /// <summary>出现过的所有事件类型（含认得的）。排查「少了哪一类事件」时用它。</summary>
    public HashSet<string> RecordedTypes { get; } = new(StringComparer.Ordinal);

    /// <summary>投影后发生了什么变化 —— 界面据此决定「更新哪一块」，而不是整页重画。</summary>
    [Flags]
    public enum Change
    {
        None = 0,
        Structure = 1,   // 新增/删除了消息
        Text = 2,        // 已有消息的正文变了（流式）
        Status = 4,      // 运行状态或标题变了
    }

    public Change Apply(DshSessionEvent evt)
    {
        var change = Change.None;
        _view.UpdatedAt = evt.Time;

        switch (evt.Type)
        {
            case DshEventType.TurnStart:
                // turn 是**从 1 开始**的（实测：一份日志里只有 `turn:1` 与 `turn:2`，
                // 没有 `turn:0`）。所以不能用它当索引去加一 —— 那样会多报一轮，
                // 而「轮数偏一」这种错在界面上看不出来，只会在对账时暴露。
                _currentTurn = evt.Data?["turn"]?.GetValue<int>() ?? _currentTurn + 1;
                _view.TurnCount = Math.Max(_view.TurnCount, _currentTurn);
                ResetStream();
                break;

            case DshEventType.UserMessage:
                change |= AppendUserMessage(evt);
                break;

            case DshEventType.AssistantChunk:
                change |= ApplyChunk(evt);
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

            // 边界事件：改变的是「进行中」的语义，界面此刻不需要新块。
            // 显式列出来而不是丢到 default —— 新增一种边界事件时，
            // 读代码的人能看出它是被判断过、而不是被漏掉的。
            case DshEventType.TurnEnd:
            case DshEventType.StepStart:
            case DshEventType.StepEnd:
                break;

            // 打包行是日志的**压缩编码**，不是事件：它们的内容与权威全文重复，
            // 重放时展开会写两遍。这里显式跳过，而不是靠「不认识」兜底。
            case DshEventType.PackedTextChunks:
            case DshEventType.PackedReasoningChunks:
            case DshEventType.PackedToolCallChunks:
                break;

            default:
                change |= AppendUnknown(evt);
                break;
        }

        return change;
    }

    private void ResetStream()
    {
        _streaming = null;
        _streamingParagraph = null;
        _streamingKey = null;
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

        // 内核自己注入的提醒也带 kind=user（实测：system-reminder 与技能清单
        // 就是另外那几条）。它们有固定标记，按标记排掉，而不是按长度猜。
        if (LooksLikeInjectedContext(text)) return Change.None;

        var message = new DshMessageView { IsMine = true, Meta = evt.Time.ToString("HH:mm") };
        message.Blocks.Add(DshBlock.Paragraph(text));
        _view.Messages.Add(message);

        // 新的一轮从这一句开始 —— 之后助手输出要开一条新的助手消息。
        ResetStream();
        return Change.Structure;
    }

    /// <summary>
    /// 内核注入的上下文长得就像用户消息，只能按内容的固定标记认。
    /// 实测见过的开头：<c>&lt;system-reminder&gt;</c>、
    /// <c>A skill is a reusable set of task-specific instructions</c>。
    /// </summary>
    private static bool LooksLikeInjectedContext(string text)
    {
        var head = text.TrimStart();
        return head.StartsWith("<system-reminder>", StringComparison.OrdinalIgnoreCase)
               || head.StartsWith("<workspace_instructions>", StringComparison.OrdinalIgnoreCase)
               || head.StartsWith("A skill is a reusable set of task-specific instructions", StringComparison.Ordinal)
               || head.StartsWith("Current runtime context.", StringComparison.Ordinal);
    }

    // ── 助手输出 ─────────────────────────────────────────────────────────

    /// <summary>取（必要时新建）本轮那条助手消息。</summary>
    private DshMessageView AssistantFor(int turn, DateTimeOffset time)
    {
        if (_assistantByTurn.TryGetValue(turn, out var existing))
        {
            existing.LastStepTime = time;
            return existing;
        }

        var message = new DshMessageView
        {
            IsMine = false,
            GroupKey = $"turn:{turn}",
            Meta = $"第 {turn} 轮",
            TurnCountHint = turn,
            FirstStepTime = time,
            LastStepTime = time,
        };
        _assistantByTurn[turn] = message;
        _view.Messages.Add(message);
        return message;
    }

    private Change ApplyChunk(DshSessionEvent evt)
    {
        if (evt.Data is not JsonObject data) return Change.None;
        if (data["chunk"] is not JsonObject chunk) return Change.None;

        var kind = chunk["type"]?.GetValue<string>();

        // finish 块是**模型失败的唯一出口**：请求没成、余额不足、鉴权失败之类
        // 都不会抛到协议层，只会在这里带一个 reason.failure。第一次端到端跑
        // 时正因为不读它，界面表现为「发出去以后什么都没有」，无从排查。
        if (string.Equals(kind, DshChunkType.Finish, StringComparison.Ordinal))
        {
            var failure = chunk["reason"]?["failure"]?["message"]?.GetValue<string>();
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

        // 重放模式不消费增量：日志里的正文由权威全文负责，
        // 增量要么被压缩成打包行、要么与权威全文重复。
        if (_replay) return Change.None;

        var delta = chunk["text"]?.GetValue<string>();
        if (string.IsNullOrEmpty(delta)) return Change.None;

        var turn = data["turn"]?.GetValue<int>() ?? Math.Max(_currentTurn, 0);
        var step = data["step"]?.GetValue<int>() ?? 0;
        var index = chunk["index"]?.GetValue<int>() ?? 0;

        // 同一步里模型可以同时产出正文块与思考块（index 不同）。不分开会让
        // 思考的增量混进正文 —— 所以流式段落按 (turn, step, index) 区分。
        var key = $"{turn}:{step}:{index}";

        var change = Change.None;
        if (_streaming is null || _streamingKey != key)
        {
            _streaming = AssistantFor(turn, evt.Time);
            _streamingKey = key;
            _streamingParagraph = DshBlock.Paragraph(string.Empty);
            _streamingParagraph.IsStreaming = true;
            _streaming.Blocks.Add(_streamingParagraph);
            change |= Change.Structure;
        }

        _streamingParagraph!.Text += delta;
        return change | Change.Text;
    }

    private Change ApplyAssistantMessage(DshSessionEvent evt)
    {
        if (evt.Data is not JsonObject data) return Change.None;
        if (data["message"] is not JsonObject message) return Change.None;

        var turn = data["turn"]?.GetValue<int>() ?? Math.Max(_currentTurn, 0);
        var step = data["step"]?.GetValue<int>() ?? 0;

        var target = AssistantFor(turn, evt.Time);
        target.StepCount++;

        var messageId = message["id"]?.GetValue<string>() ?? $"{turn}:{step}";
        _messagesById[messageId] = target;

        // 权威全文取代同一步的流式增量。整块重建那一小段而不是整条消息：
        // 一步里可能先出正文、再出工具调用，把整条清空会把前面的内容抹掉。
        DropStreamingParagraph(target);

        foreach (var item in message["content"] as JsonArray ?? new JsonArray())
        {
            if (item is not JsonObject block) continue;

            switch (block["type"]?.GetValue<string>())
            {
                case "text":
                    var text = block["text"]?.GetValue<string>();
                    if (!string.IsNullOrEmpty(text)) target.Blocks.Add(DshBlock.Paragraph(text!));
                    break;

                case "tool-call":
                    var callId = block["id"]?.GetValue<string>();
                    var name = block["name"]?.GetValue<string>() ?? "工具";
                    var args = block["arguments"]?.ToString() ?? "{}";
                    var tool = NewToolBlock(name, callId, args);
                    target.Blocks.Add(tool);
                    if (callId is not null) _toolsByCallId[callId] = tool;
                    break;

                case "reasoning":
                    // 思考块收进来、**默认折叠**。实测一份 238 步的会话里
                    // reasoning 比正文还多；全展开会把正文彻底淹掉。
                    var reasoning = block["text"]?.GetValue<string>();
                    if (!string.IsNullOrEmpty(reasoning))
                    {
                        var rb = DshBlock.Paragraph(reasoning!);
                        rb.Kind = DshBlockKind.Reasoning;
                        rb.SourceType = "reasoning";
                        target.Blocks.Add(rb);
                    }
                    break;

                // 将来出现新的块类型时**保留原文而不是丢掉** ——
                // 丢掉之后「这里发生过一件事」这个信息就没了。
                default:
                    var raw = DshBlock.Paragraph(item.ToJsonString(Transport.JsonRpcLineTransport.JsonOptions));
                    raw.Kind = DshBlockKind.Unknown;
                    raw.SourceType = $"message.content:{block["type"]?.GetValue<string>()}";
                    target.Blocks.Add(raw);
                    break;
            }
        }

        ResetStream();
        return Change.Structure;
    }

    /// <summary>
    /// 丢掉还在流的那一段 —— 它的权威版本马上要进来了。
    ///
    /// 只丢标记为 <see cref="DshBlock.IsStreaming"/> 的块，不碰已经定稿的段。
    /// </summary>
    private static void DropStreamingParagraph(DshMessageView message)
    {
        for (var i = message.Blocks.Count - 1; i >= 0; i--)
        {
            if (message.Blocks[i].IsStreaming) message.Blocks.RemoveAt(i);
        }
    }

    // ── 工具 ─────────────────────────────────────────────────────────────

    private static DshBlock NewToolBlock(string name, string? callId, string arguments)
    {
        var block = new DshBlock
        {
            Kind = DshBlockKind.Tool,
            ToolName = name,
            CallId = callId,
            ToolArguments = arguments,
            RawInput = arguments,
            Title = ToolTitle(name, arguments),
        };

        // 原始参数留在子块里：解析出来的是给人看的标题，原文是排查时唯一能信的
        // 东西（每个工具的参数形状都不一样）。
        var argBlock = DshBlock.Paragraph(arguments);
        argBlock.Kind = DshBlockKind.Raw;
        argBlock.SourceType = "tool.arguments";
        block.Children.Add(argBlock);

        return block;
    }

    /// <summary>
    /// 工具卡的一行标题。
    ///
    /// 参数是 JSON 字符串，里面对人最有用的字段因工具而异 ——
    /// 与其猜结构，不如按**已知的几个字段名**取，取不到就退回工具名本身。
    /// 猜错的代价是一行误导性的标题，那比没有标题更糟。
    /// </summary>
    private static string ToolTitle(string name, string arguments)
    {
        try
        {
            if (JsonNode.Parse(arguments) is JsonObject args)
            {
                foreach (var field in new[]
                         {
                             "command", "file_path", "path", "pattern", "url",
                             "query", "description", "prompt", "goal", "job_id",
                         })
                {
                    var value = args[field]?.GetValue<string>();
                    if (string.IsNullOrWhiteSpace(value)) continue;
                    value = value.Replace('\r', ' ').Replace('\n', ' ').Trim();
                    return value.Length <= 90 ? value : value[..90] + "…";
                }
            }
        }
        catch (System.Text.Json.JsonException)
        {
            // 参数不是 JSON（工具可能收到自由文本）——回退到工具名。
        }

        return name;
    }

    private Change ApplyToolResult(DshSessionEvent evt)
    {
        if (evt.Data is not JsonObject data) return Change.None;
        if (data["message"] is not JsonObject message) return Change.None;

        var first = (message["content"] as JsonArray)?.FirstOrDefault() as JsonObject;
        var callId = first?["toolCallId"]?.GetValue<string>();
        if (callId is null) return Change.None;

        var text = JoinTextBlocks(first!["content"]);
        var isError = first?["isError"]?.GetValue<bool>() ?? false;

        if (!_toolsByCallId.TryGetValue(callId, out var tool))
        {
            // 结果先到、调用没到 —— 不该发生，但**重放被截断的日志时会出现**。
            // 建一个只有结果的工具块，比丢掉结果好：丢掉会让「工具跑过了」消失。
            var target = AssistantFor(data["turn"]?.GetValue<int>() ?? Math.Max(_currentTurn, 0), evt.Time);
            tool = NewToolBlock(first?["name"]?.GetValue<string>() ?? "工具", callId, "{}");
            target.Blocks.Add(tool);
            _toolsByCallId[callId] = tool;
        }

        tool.Text = text;
        tool.IsError = isError;
        tool.HasResult = true;

        var resultBlock = DshBlock.Paragraph(text);
        resultBlock.Kind = DshBlockKind.Raw;
        resultBlock.SourceType = "tool.result";
        tool.Children.Add(resultBlock);

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
