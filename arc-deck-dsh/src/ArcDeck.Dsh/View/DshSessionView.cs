namespace ArcDeck.Dsh.View;

/// <summary>会话视图里的一个块的种类 —— 只列界面真正会画的那几种。</summary>
public enum DshBlockKind
{
    /// <summary>正文段落（含 Markdown 原文，渲染交给界面）。</summary>
    Text,

    /// <summary>工具调用：一行标题 + 可展开的输出。</summary>
    Tool,

    /// <summary>上游新增、本客户端还不认识的事件。保留原文，画成一条可展开的原始条目。</summary>
    Unknown,
}

/// <summary>一个块。字段刻意做成扁平的可选值，而不是继承体系 —— 三种块的差异只有两三个字段。</summary>
public sealed class DshBlock
{
    /// <summary>
    /// 块的种类。可写而不是 init-only：错误与未知两类块是先建出正文再定类的
    /// （它们的构造路径与普通段落相同，只有分类不同）。
    /// </summary>
    public DshBlockKind Kind { get; set; }

    /// <summary>正文或工具输出（原文，可能是 Markdown）。</summary>
    public string Text { get; set; } = string.Empty;

    /// <summary>工具名。仅 <see cref="DshBlockKind.Tool"/> 有值。</summary>
    public string? ToolName { get; set; }

    /// <summary>工具调用 id —— 结果回来时按它配对。仅 Tool 有值。</summary>
    public string? CallId { get; set; }

    /// <summary>工具参数原文（JSON 字符串）。仅 Tool 有值。</summary>
    public string? ToolArguments { get; set; }

    /// <summary>结果是否标记为错误。仅 Tool 有值。</summary>
    public bool IsError { get; set; }

    /// <summary>工具结果是否已经回来。没回来时界面显示「进行中」。</summary>
    public bool HasResult { get; set; }

    /// <summary>上游事件类型 —— 用于未知块，也用于排查。</summary>
    public string? SourceType { get; set; }

    public static DshBlock Paragraph(string text) => new() { Kind = DshBlockKind.Text, Text = text };
}

/// <summary>会话里的一条消息。</summary>
public sealed class DshMessageView
{
    public bool IsMine { get; init; }

    /// <summary>助手消息的模型/耗时注解；自己的消息只有时间。</summary>
    public string Meta { get; set; } = string.Empty;

    public List<DshBlock> Blocks { get; } = new();

    /// <summary>流式追加时用来复用同一条消息的键（无则每次新建）。</summary>
    public string? GroupKey { get; set; }
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
    /// 列表里那一行摘要。取最后一条可见消息的第一段正文，压成一行。
    /// 空会话给一句明确的占位，而不是空字符串 —— 空行看起来像渲染坏了。
    /// </summary>
    public string Preview
    {
        get
        {
            for (var i = Messages.Count - 1; i >= 0; i--)
            {
                foreach (var block in Messages[i].Blocks)
                {
                    if (block.Kind != DshBlockKind.Text) continue;
                    var text = block.Text.Replace('\r', ' ').Replace('\n', ' ').Trim();
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
