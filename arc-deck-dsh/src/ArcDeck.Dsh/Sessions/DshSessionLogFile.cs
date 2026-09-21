using System.Text;
using System.Text.Json.Nodes;
using ZstdSharp;

namespace ArcDeck.Dsh.Sessions;

/// <summary>
/// 会话日志的落盘格式。
///
/// 实测（本机 DSH Desktop 0.8.2 的 harness home）：
///
///     &lt;DSH_HOME&gt;/sessions/&lt;工作区分组&gt;/&lt;会话 id&gt;/session.jsonl.zstd
///
/// **是 zstd，不是明文**，这一点值得写下来：文件名里写着 <c>.jsonl</c>，
/// 直接按文本读会得到一堆二进制噪声，而且报错是「JSON 解析失败」——
/// 指向的方向完全不对。
///
/// 解压后是按 <c>\n</c> 分行的记录：
///
///     第 1 行  <c>{"type":"session","version":0,"id":…,"createdAt":…,"cwd":…,…}</c>
///     之后每行  一帧会话事件，形状与 <c>session.event</c> 通知里的 event 完全一致
///
/// **首行不是事件**，它没有 <c>seq</c>。把它当事件喂给投影会让 <c>seq</c> 从 0 起
/// 错位，所以解析必须显式分叉。
/// </summary>
public static class DshSessionLogFile
{
    public const string HeaderType = "session";
    public const string FileName = "session.jsonl.zstd";

    /// <summary>
    /// 解压整个日志。
    ///
    /// 用 <see cref="DecompressionStream"/> 而不是一次性 API：日志是**追加写**的，
    /// 每追加一段就是一个新的 zstd 帧。实测确认过 —— 本机一份 1.5 MB 的日志
    /// 解压出 3.9 MB / 4733 行，与 zstd 命令行**逐字节一致**。
    /// 只解第一帧的 API 会静默丢掉后半段，表现是「重载的会话少了最近几条消息」，
    /// 看起来像投影逻辑的 bug。
    /// </summary>
    public static string ReadText(string path)
    {
        using var input = File.OpenRead(path);

        // checkEndOfStream:false —— 拼接流的“末尾”由我们自己判断；
        // 让第一帧的帧尾就抛错会把一份完好的日志判成坏的。
        using var decompression = new DecompressionStream(input, bufferSize: 1 << 16, checkEndOfStream: false);
        using var output = new MemoryStream();
        decompression.CopyTo(output);

        return Encoding.UTF8.GetString(output.GetBuffer(), 0, (int)output.Length);
    }

    /// <summary>
    /// 切分出的行，以及**可能存在的截断末行**。
    ///
    /// 一个正在被内核追加写的日志，末尾可能是半行。这不是错误，但必须说出来 ——
    /// 静默丢掉它会让「少了最后一条消息」变成不可解释的现象。
    ///
    /// 判据只看「末尾有没有换行」，不去猜内容：猜内容会把合法的紧凑 JSON
    /// 误判成截断（`{...}` 与 `{...` 无法从形状上区分）。
    /// </summary>
    public readonly record struct LogLines(IReadOnlyList<string> Lines, string? TruncatedTail)
    {
        public bool HasTruncatedTail => !string.IsNullOrEmpty(TruncatedTail);
    }

    public static LogLines SplitLines(string text)
    {
        var raw = text.Split('\n');
        string? truncated = null;
        var count = raw.Length;

        if (text.Length > 0 && text[^1] != '\n')
        {
            truncated = raw[^1];
            count--;
        }

        var lines = new List<string>(count);
        for (var i = 0; i < count; i++)
        {
            if (raw[i].Length > 0) lines.Add(raw[i]);
        }

        return new LogLines(lines, truncated);
    }

    /// <summary>解析首行那一帧会话头。解析不出来返回 null。</summary>
    public static DshSessionHeader? ParseHeader(string line)
    {
        JsonNode? node;
        try { node = JsonNode.Parse(line); }
        catch (System.Text.Json.JsonException) { return null; }

        if (node is not JsonObject obj) return null;
        if (obj["type"]?.GetValue<string>() != HeaderType) return null;

        var id = obj["id"]?.GetValue<string>();
        if (string.IsNullOrEmpty(id)) return null;

        var createdAt = obj["createdAt"]?.GetValue<long>() ?? 0;
        return new DshSessionHeader(
            SessionId: id!,
            Workspace: obj["cwd"]?.GetValue<string>() ?? string.Empty,
            CreatedAt: createdAt > 0
                ? DateTimeOffset.FromUnixTimeMilliseconds(createdAt).ToLocalTime()
                : DateTimeOffset.MinValue,
            AgentPreset: obj["agentPreset"]?.GetValue<string>());
    }
}

/// <summary>日志首行那一帧会话头。</summary>
public sealed record DshSessionHeader(
    string SessionId,
    string Workspace,
    DateTimeOffset CreatedAt,
    string? AgentPreset);
