namespace ArcDeck.Dsh.View;

/// <summary>会话视图里的一个块的种类 —— 只列界面真正会画的那几种。</summary>
public enum DshBlockKind
{
    /// <summary>正文段落（含 Markdown 原文，渲染交给界面）。</summary>
    Text,

    /// <summary>思考过程。默认折叠 —— 实测一份 238 步的会话里它比正文还多。</summary>
    Reasoning,

    /// <summary>工具调用：一行标题 + 可展开的输入输出与子块。</summary>
    Tool,

    /// <summary>原始文本（工具参数、工具输出、未知事件的 JSON）。画成等宽、可复制。</summary>
    Raw,

    /// <summary>上游新增、本客户端还不认识的东西。保留原文，可展开。</summary>
    Unknown,
}

/// <summary>
/// 一个块。
///
/// 字段刻意做成扁平的可选值而不是继承体系：五种块的差异只有两三个字段，
/// 而继承体系会让「界面按类型分派」这件事多一层（界面只该看
/// <see cref="Kind"/> 与它关心的那几个字段）。
/// </summary>
public sealed class DshBlock
{
    /// <summary>
    /// 块的种类。可写而不是 init-only：错误与未知两类块是先建出正文再定类的
    /// （它们的构造路径与普通段落相同，只有分类不同）。
    /// </summary>
    public DshBlockKind Kind { get; set; }

    /// <summary>正文或工具输出（原文，可能是 Markdown）。</summary>
    public string Text { get; set; } = string.Empty;

    /// <summary>工具卡的一行标题（从参数里提取，见 <c>DshProjection.ToolTitle</c>）。仅 Tool 有值。</summary>
    public string? Title { get; set; }

    /// <summary>工具名。仅 Tool 有值。</summary>
    public string? ToolName { get; set; }

    /// <summary>工具调用 id —— 结果回来时按它配对。仅 Tool 有值。</summary>
    public string? CallId { get; set; }

    /// <summary>工具参数原文（JSON 字符串）。仅 Tool 有值。</summary>
    public string? ToolArguments { get; set; }

    /// <summary>工具参数原文。与 <see cref="ToolArguments"/> 同源，但语义是「排查用」。仅 Tool 有值。</summary>
    public string? RawInput { get; set; }

    /// <summary>结果是否标记为错误。仅 Tool 有值。</summary>
    public bool IsError { get; set; }

    /// <summary>工具结果是否已经回来。没回来时界面显示「进行中」。</summary>
    public bool HasResult { get; set; }

    /// <summary>
    /// 还在流式追加中。界面据此决定「要不要保留这一段的引用并就地更新」——
    /// 权威全文一到，这一段会被 <c>DshProjection</c> 移除。
    /// </summary>
    public bool IsStreaming { get; set; }

    /// <summary>子块。仅 Tool 有值（参数原文 + 结果原文）。</summary>
    public List<DshBlock> Children { get; } = new();

    /// <summary>上游事件类型或块来源 —— 用于未知块，也用于排查。</summary>
    public string? SourceType { get; set; }

    public static DshBlock Paragraph(string text) => new() { Kind = DshBlockKind.Text, Text = text };
}

/// <summary>
/// 会话里的一条消息。
///
/// **一条消息 = 一轮（turn）**，不是一步（step）。见 <c>DshProjection</c> 的类注释。
/// </summary>
public sealed class DshMessageView
{
    public bool IsMine { get; init; }

    /// <summary>
    /// 助手消息的注解：第几轮、跑了几步、起止时刻。
    ///
    /// 一轮可能跑很久（实测一次 238 步），只给一个时间会误导 ——
    /// 所以起止都给，只在跨度超过一分钟时才显示「→」。
    /// </summary>
    public string Meta
    {
        get
        {
            if (IsMine) return MetaText;

            var span = LastStepTime - FirstStepTime;
            var time = span.TotalMinutes >= 1
                ? $"{FirstStepTime:HH:mm} → {LastStepTime:HH:mm}"
                : $"{FirstStepTime:HH:mm}";

            return StepCount > 1
                ? $"{MetaText} · {time} · {StepCount} 步"
                : $"{MetaText} · {time}";
        }
        set => MetaText = value;
    }

    /// <summary>轮次标签（「第 1 轮」），由投影写入。</summary>
    public string MetaText { get; set; } = string.Empty;

    public List<DshBlock> Blocks { get; } = new();

    /// <summary>复用同一条消息的键（助手消息是 <c>turn:N</c>）。</summary>
    public string? GroupKey { get; set; }

    /// <summary>这一轮第一步/最后一步的时间 —— 一轮可能跑很久，只给一个时间会误导。</summary>
    public DateTimeOffset FirstStepTime { get; set; }

    public DateTimeOffset LastStepTime { get; set; }

    /// <summary>这一轮里模型被调用了几次。界面用它给出「跑了多少步」的量感。</summary>
    public int StepCount { get; set; }

    /// <summary>
    /// 这条消息属于第几轮（助手消息才有值）。
    ///
    /// 刻意留一个**结构化**的轮次，而不是让人从 <see cref="Meta"/> 那串文案里
    /// 反解 —— 文案会改，断言不该跟着改。
    /// </summary>
    public int TurnCountHint { get; set; }

    /// <summary>这一条里属于某一种块的个数 —— 界面摘要行用它，避免遍历。</summary>
    public int CountOf(DshBlockKind kind)
    {
        var n = 0;
        foreach (var b in Blocks)
        {
            if (b.Kind == kind) n++;
        }
        return n;
    }
}

/// <summary>
/// 一个会话的可见投影。
///
/// 它是**纯数据**：不认识 XAML，也不认识 JSON-RPC。
/// 刻意做成这样，是因为「哪一种事件产生哪一个块」是本层最容易错的地方，
/// 而纯数据可以在没有内核、没有窗口的情况下用一段真实事件日志回放验证。
/// </summary>
public sealed class DshSessionView
{
    public string SessionId { get; init; } = string.Empty;

    public string Title { get; set; } = "新会话";

    public string Workspace { get; set; } = string.Empty;

    public List<DshMessageView> Messages { get; } = new();

    /// <summary>agent 是否在跑。来自 <c>session.status</c>，不是猜的。</summary>
    public bool IsRunning { get; set; }

    public int TurnCount { get; set; }

    /// <summary>这一会话实际用的 provider · model（来自 <c>request/header</c>，不是客户端假定的）。</summary>
    public string? ModelLabel { get; set; }

    public DateTimeOffset UpdatedAt { get; set; } = DateTimeOffset.Now;

    /// <summary>
    /// 列表里那一行摘要。取最后一条可见消息的**最后一段正文**，压成一行。
    ///
    /// 取最后而不是第一段：助手消息现在是一轮一条，第一段往往是这一轮开头的
    /// 一句交代，而列表上想看的是「它最后停在哪」。
    /// 空会话给一句明确的占位，而不是空字符串 —— 空行看起来像渲染坏了。
    /// </summary>
    public string Preview
    {
        get
        {
            for (var i = Messages.Count - 1; i >= 0; i--)
            {
                var blocks = Messages[i].Blocks;
                for (var j = blocks.Count - 1; j >= 0; j--)
                {
                    if (blocks[j].Kind != DshBlockKind.Text) continue;
                    var text = blocks[j].Text.Replace('\r', ' ').Replace('\n', ' ').Trim();
                    if (text.Length == 0) continue;
                    return text.Length <= 80 ? text : text[..80] + "…";
                }
            }
            return "还没有消息。";
        }
    }

    /// <summary>会话长度 —— 界面上的「N 轮」。轮次由 <c>turn/start</c> 计。</summary>
    public string Subtitle => $"{Workspace} · {TurnCount} 轮";
}
