using System.Text.Json.Nodes;
using ArcDeck.Dsh.View;

namespace ArcDeck.Dsh.Sessions;

/// <summary>磁盘上一个会话文件的摘要 —— 列表页要的就是这些。</summary>
public sealed record DshSessionLogInfo(
    string SessionId,
    string Workspace,
    DateTimeOffset CreatedAt,
    DateTimeOffset ModifiedAt,
    long CompressedBytes,
    string Path);

/// <summary>一次重载的结果，连同「读的时候发生了什么」。</summary>
public sealed class DshLoadedSession
{
    public required DshSessionView View { get; init; }
    public required DshSessionHeader Header { get; init; }

    /// <summary>日志里的事件帧数（不含首行会话头，也不含打包行）。</summary>
    public required int EventCount { get; init; }

    /// <summary>
    /// 跳过的打包行数（<c>text-chunks</c> / <c>reasoning-chunks</c> / <c>tool-call-chunks</c>）。
    ///
    /// 报出来是刻意的：这个数字很大说明日志被压缩过（实测一份 5127 行的日志里
    /// 有 2297 行是打包行），而「为什么不展开它们」是重放正确性的关键决定 ——
    /// 让它可见，比让它悄悄发生好。
    /// </summary>
    public required int PackedRowCount { get; init; }

    /// <summary>重放期间认不出来的事件类型（含内核内部簿记）。</summary>
    public required IReadOnlyCollection<string> UnhandledTypes { get; init; }

    /// <summary>末尾是不是半行 —— 内核正在写这个会话时会是 true，不是错误。</summary>
    public required bool TailTruncated { get; init; }
}

/// <summary>
/// 把已经落盘的会话日志读回成界面能画的视图。
///
/// ## 为什么要「重放」而不是直接读消息
///
/// 日志里没有「消息」这种东西 —— 只有事件。一条助手消息在日志里是
/// 一步的 <c>text-delta</c> 若干帧 + 一帧 <c>assistant/message</c>；
/// 哪个显示、哪个不显示，全部取决于折叠规则。
///
/// 所以重载**必须复用同一个 <see cref="DshProjection"/>**。
/// 另写一份「读日志」的逻辑会立刻产生第二个真相源，而两个真相源的差异
/// 只在特定会话上才显形（比如带工具调用的那几步）。实时与重载走到同一条
/// 折叠路径上，「看到的和存下来的一致」这件事才有保证。
/// </summary>
public static class DshSessionReader
{
    /// <summary>会话目录名固定的分组层级：<c>sessions/&lt;工作区分组&gt;/&lt;会话 id&gt;/</c>。</summary>
    public static string SessionsRoot(string harnessHome) =>
        Path.Combine(harnessHome, "sessions");

    /// <summary>
    /// 列出所有会话文件，**最新的在前**。
    ///
    /// 只读首行（会话头）与文件元数据，不解压全文 —— 一次列目录要把
    /// 几 MB 的日志全解开是不可接受的，而列表页只需要标题以外的那几个字段。
    /// 所以标题**不在这里**：它要么来自文件名，要么等真正打开时才知道。
    /// 这个取舍是刻意的，别为了「列表上显示真标题」把全文解压搬到这一层。
    /// </summary>
    public static IReadOnlyList<DshSessionLogInfo> Enumerate(string harnessHome, string? workspace = null)
    {
        var root = SessionsRoot(harnessHome);
        if (!Directory.Exists(root)) return Array.Empty<DshSessionLogInfo>();

        var results = new List<DshSessionLogInfo>();

        foreach (var file in Directory.EnumerateFiles(root, DshSessionLogFile.FileName, SearchOption.AllDirectories))
        {
            var info = new FileInfo(file);
            var header = TryReadHeader(file);
            if (header is null) continue;

            if (workspace is not null
                && !string.Equals(
                    Path.GetFullPath(header.Workspace).TrimEnd('\\', '/'),
                    Path.GetFullPath(workspace).TrimEnd('\\', '/'),
                    StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            results.Add(new DshSessionLogInfo(
                SessionId: header.SessionId,
                Workspace: header.Workspace,
                CreatedAt: header.CreatedAt,
                ModifiedAt: info.LastWriteTime,
                CompressedBytes: info.Length,
                Path: file));
        }

        return results.OrderByDescending(x => x.ModifiedAt).ToList();
    }

    /// <summary>只解压首行的字节前缀，读会话头。整份日志不解。</summary>
    public static DshSessionHeader? TryReadHeader(string path)
    {
        try
        {
            // 只喂前 64 KB 给解压器：会话头在第一帧的最前面，
            // 而第一帧通常就等于首次写入的那一小段。
            using var input = File.OpenRead(path);
            var prefix = new byte[Math.Min(64 * 1024, input.Length)];
            var read = input.Read(prefix, 0, prefix.Length);

            using var slice = new MemoryStream(prefix, 0, read);
            using var decompression = new ZstdSharp.DecompressionStream(
                slice, bufferSize: 1 << 16, checkEndOfStream: false);

            var buffer = new byte[8192];
            var got = decompression.Read(buffer, 0, buffer.Length);
            if (got <= 0) return null;

            var text = System.Text.Encoding.UTF8.GetString(buffer, 0, got);
            var newline = text.IndexOf('\n');
            var firstLine = newline < 0 ? text : text[..newline];

            return DshSessionLogFile.ParseHeader(firstLine);
        }
        catch
        {
            // 正在写、文件被占用、压缩段还没封口 —— 都会走到这里。
            // 列表页不该因为一个坏文件整个失败。
            return null;
        }
    }

    /// <summary>
    /// 重放一份日志，得到一个可以直接绑到界面上的视图。
    ///
    /// 大日志（实测本机最大的一份：1.5 MB 压缩 / 3.9 MB 解压 / 4733 帧）
    /// 是同步跑的：97 ms 解压 + 投影。放在 UI 线程上会卡一下，但目前不值得
    /// 为它引入异步与进度 —— 记在这里，等列表页真的要一次开十个会话时再说。
    /// </summary>
    public static DshLoadedSession Load(string path) => Load(path, replay: true);

    /// <summary>
    /// 对照实验用的入口：**按「最朴素的做法」重放** —— 消费增量、并把打包行
    /// 展开成增量（也就是两处判断都不做）。
    ///
    /// 存在的唯一理由是让不变式断言有牙齿。用 <c>--no-replay</c> 跑同一份日志，
    /// 「正文重复」那条**必须**报错；不报错就说明断言写错了地方。
    /// 产品路径永远走 <see cref="Load(string)"/>。
    /// </summary>
    public static DshLoadedSession LoadRaw(string path) => Load(path, replay: false, expandPacked: true);

    public static DshLoadedSession Load(string path, bool replay, bool expandPacked = false)
    {
        var text = DshSessionLogFile.ReadText(path);
        var split = DshSessionLogFile.SplitLines(text);

        if (split.Lines.Count == 0)
            throw new InvalidDataException($"会话日志是空的：{path}");

        var header = DshSessionLogFile.ParseHeader(split.Lines[0])
            ?? throw new InvalidDataException($"会话日志首行不是会话头：{path}");

        var view = new DshSessionView
        {
            SessionId = header.SessionId,
            Workspace = header.Workspace,
            Title = "会话",
        };

        // **replay: true**（默认）—— 见 DshProjection 的类注释。
        // 日志里的正文增量大多被压成打包行（实测：一份 5127 行的日志里，
        // 真正的 text-delta 只有 1 帧，其余正文全在 text-chunks 里），
        // 展开它们会与权威全文重复。重放只认权威全文。
        var projection = new DshProjection(view, replay, expandPacked);
        var packedRows = 0;

        var events = 0;
        for (var i = 1; i < split.Lines.Count; i++)
        {
            var line = split.Lines[i];

            JsonNode? node;
            try { node = JsonNode.Parse(line); }
            catch (System.Text.Json.JsonException)
            {
                // 一行坏掉不该让整份会话打不开。记进 UnhandledTypes 之外的地方
                // 会更好，但先把「能不能读」这件事做对。
                continue;
            }

            if (node is not JsonObject obj) continue;
            var type = obj["type"]?.GetValue<string>();
            if (string.IsNullOrEmpty(type)) continue;

            // 打包行单独计数，不算进「事件帧数」—— 它们不是事件，
            // 把它们混进去会让「这份日志有多少帧」这个数字失去意义。
            if (type is DshEventType.PackedTextChunks
                     or DshEventType.PackedReasoningChunks
                     or DshEventType.PackedToolCallChunks)
            {
                packedRows++;
                continue;
            }

            var seq = obj["seq"]?.GetValue<long>() ?? -1;
            var timeMs = obj["time"]?.GetValue<long>() ?? 0;

            projection.Apply(new DshSessionEvent(
                SessionId: header.SessionId,
                Type: type!,
                Sequence: seq,
                Time: timeMs > 0
                    ? DateTimeOffset.FromUnixTimeMilliseconds(timeMs).ToLocalTime()
                    : header.CreatedAt,
                Data: obj["data"],
                Raw: obj));
            events++;
        }

        return new DshLoadedSession
        {
            View = view,
            Header = header,
            EventCount = events,
            PackedRowCount = packedRows,
            UnhandledTypes = projection.RecordedTypes,
            TailTruncated = split.HasTruncatedTail,
        };
    }

    /// <summary>
    /// 把「实时看到的」与「重放出来的」逐块比一遍。
    ///
    /// **这是本层唯一能证明「看到的就是存下来的」的手段。** 之前所有关于重放的
    /// 说法都建立在「两条路径共用同一个 DshProjection」这个论证上 ——
    /// 论证是对的，但论证不等于证据：只要有一处（比如重放额外跳过打包行、
    /// 或者实时模式多消费了一类增量）不对称，界面上就会出现「重开就变样」。
    ///
    /// 比对刻意**不看时间戳**（实时与重放取的 time 字段同源，但仍可能差毫秒），
    /// 只看结构与文本 —— 那才是用户能感知到的东西。
    /// </summary>
    public static LoadedSessionCompare Compare(DshSessionView live, DshSessionView replayed)
    {
        var diffs = new List<string>();

        if (live.Messages.Count != replayed.Messages.Count)
        {
            diffs.Add($"消息条数：实时 {live.Messages.Count} ≠ 重放 {replayed.Messages.Count}");
        }
        if (live.TurnCount != replayed.TurnCount)
        {
            diffs.Add($"轮次：实时 {live.TurnCount} ≠ 重放 {replayed.TurnCount}");
        }

        var n = Math.Min(live.Messages.Count, replayed.Messages.Count);
        for (var i = 0; i < n; i++)
        {
            var a = live.Messages[i];
            var b = replayed.Messages[i];

            if (a.IsMine != b.IsMine)
            {
                diffs.Add($"#{i} 归属不同：实时 {(a.IsMine ? "我" : "内核")} ≠ 重放 {(b.IsMine ? "我" : "内核")}");
                continue;
            }

            var aBlocks = Describe(a);
            var bBlocks = Describe(b);
            if (aBlocks != bBlocks)
            {
                diffs.Add($"#{i} 块序列不同：\n      实时 {aBlocks}\n      重放 {bBlocks}");
            }
        }

        return new LoadedSessionCompare(diffs);
    }

    /// <summary>
    /// 把一个消息压成可比较、可读的一行：块的种类序列 + 每种块的文本长度。
    ///
    /// 用长度而不是全文：全文会是几百 KB，而**长度不同就足以说明内容不同**；
    /// 长度相同而内容不同极不可能（那是同一批事件折叠出来的）。
    /// </summary>
    private static string Describe(DshMessageView message)
    {
        var parts = new List<string>(message.Blocks.Count);
        foreach (var block in message.Blocks)
        {
            var tag = block.Kind switch
            {
                DshBlockKind.Tool => $"{block.ToolName ?? "tool"}({block.Text.Length}{(block.HasResult ? "" : ",未回")})",
                DshBlockKind.Reasoning => $"think({block.Text.Length})",
                DshBlockKind.Raw => $"raw({block.Text.Length})",
                DshBlockKind.Unknown => $"unk({block.Text.Length})",
                _ => $"text({block.Text.Length})",
            };
            parts.Add(tag);
        }
        return $"{message.Blocks.Count} 块 [{(parts.Count == 0 ? "空" : string.Join(" ", parts))}]";
    }
}

/// <summary>两条投影路径的比对结果。</summary>
public sealed class LoadedSessionCompare
{
    public LoadedSessionCompare(IReadOnlyList<string> differences) => Differences = differences;

    public IReadOnlyList<string> Differences { get; }

    public bool IsEquivalent => Differences.Count == 0;
}
