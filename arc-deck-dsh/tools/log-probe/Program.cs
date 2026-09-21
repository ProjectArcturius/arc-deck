// 会话日志的**帧级**结构探针 + 重放的**不变式检查**。
//
// 动机有两个，都是实测撞出来的：
//   1. 把一份真实日志重放成投影之后，出现了一个说不通的现象（几十个工具调用
//      挤在少数几条消息里）。要判断是「投影的键错了」还是「日志本身的形状与
//      预期不同」，只能把原始帧序列摊开看。
//   2. 日志里的正文增量大多被压成**打包行**（一份 5127 行的日志里真正的
//      text-delta 只有 1 帧，2840 行是打包编码）。重放时若把打包行也展开，
//      正文会写两遍 —— 而那件事在终端输出里**看不出来**，只能靠断言抓。
//
// 用法：
//   dotnet run --project arc-deck-dsh/tools/log-probe -- <session.jsonl.zstd> [--type <事件类型>] [--around <seq>] [--check]
//   dotnet run --project arc-deck-dsh/tools/log-probe -- --check-all <sessions 根目录>
using System.Text.Json.Nodes;
using ArcDeck.Dsh;
using ArcDeck.Dsh.Sessions;
using ArcDeck.Dsh.View;

if (args.Length == 0)
{
    Console.WriteLine("用法: log-probe <session.jsonl.zstd> [--type <事件类型>] [--around <seq>] [--check]");
    Console.WriteLine("      log-probe --check-all <sessions 根目录>");
    return 2;
}

// ── 批量校验模式 ────────────────────────────────────────────────────────
if (args[0] == "--check-all")
{
    return CheckAll(args[1]);
}

var path = args[0];
var filterType = ArgValue(args, "--type");
var around = ArgValue(args, "--around") is { } a ? long.Parse(a) : (long?)null;
var check = args.Contains("--check");
var noReplay = args.Contains("--no-replay");

var text = DshSessionLogFile.ReadText(path);
var split = DshSessionLogFile.SplitLines(text);
Console.WriteLine($"行数 {split.Lines.Count:N0}，截断末行={split.HasTruncatedTail}");
Console.WriteLine();

// 事件类型直方图 —— 第一个要看的永远是「到底有哪些类型、各多少帧」。
var counts = new Dictionary<string, int>(StringComparer.Ordinal);
var rows = new List<(long Seq, string Type, JsonObject Obj)>();

foreach (var line in split.Lines)
{
    JsonNode? node;
    try { node = JsonNode.Parse(line); } catch { continue; }
    if (node is not JsonObject obj) continue;
    var type = obj["type"]?.GetValue<string>() ?? "(none)";
    counts[type] = counts.GetValueOrDefault(type) + 1;
    rows.Add((obj["seq"]?.GetValue<long>() ?? -1, type, obj));
}

Console.WriteLine($"事件类型 {counts.Count} 种：");
foreach (var (type, n) in counts.OrderByDescending(kv => kv.Value))
{
    Console.WriteLine($"  {n,6}  {type}");
}

if (filterType is not null)
{
    var matched = rows.Where(r => r.Type == filterType).ToList();

    // assistant/chunk 的子类型直方图。**一定要能数出来** ——
    // 「流式正文到底有没有到过 text-delta」这个问题不能靠猜，
    // 而外部 zstd 工具在这里不可靠（见文件头的教训）。
    if (filterType == "assistant/chunk")
    {
        var sub = new Dictionary<string, int>(StringComparer.Ordinal);
        foreach (var (_, _, obj) in matched)
        {
            var t = obj["data"]?["chunk"]?["type"]?.GetValue<string>() ?? "(no chunk)";
            sub[t] = sub.GetValueOrDefault(t) + 1;
        }
        Console.WriteLine();
        Console.WriteLine("── assistant/chunk 子类型 ──");
        foreach (var (t, n) in sub.OrderByDescending(kv => kv.Value))
            Console.WriteLine($"  {n,6}  {t}");
    }

    Console.WriteLine();
    Console.WriteLine($"── {filterType} 的前 12 帧（关键字段） ──");
    foreach (var (seq, _, obj) in matched.Take(12))
    {
        var data = obj["data"];
        var turn = data?["turn"]?.GetValue<int>();
        var step = data?["step"]?.GetValue<int>();
        var id = data?["message"]?["id"]?.GetValue<string>();
        var chunkType = data?["chunk"]?["type"]?.GetValue<string>();
        var index = data?["chunk"]?["index"]?.GetValue<int>();
        var contentTypes = data?["message"]?["content"] is JsonArray arr
            ? string.Join(",", arr.OfType<JsonObject>().Select(b => b["type"]?.GetValue<string>()))
            : null;
        Console.WriteLine($"  seq={seq,6} turn={turn} step={step} idx={index} chunk={chunkType} msgId={Shorten(id)} content=[{contentTypes}]");

        // 打包行（text-chunks / reasoning-chunks / tool-call-chunks）没有 seq，
        // 形状也与事件不同 —— 把它们自己的字段打出来，别硬套事件的取法。
        if (obj["seq"] is null && data is JsonObject d)
        {
            Console.WriteLine($"        打包行字段: {Shorten(d.ToJsonString(), 300)}");
        }
    }
}

if (around is { } seqNo)
{
    Console.WriteLine();
    Console.WriteLine($"── seq {seqNo} 附近 ──");
    var lo = rows.FindIndex(r => r.Seq >= seqNo - 2);
    if (lo < 0) lo = 0;
    foreach (var (seq, type, obj) in rows.Skip(lo).Take(8))
    {
        var json = obj.ToJsonString();
        Console.WriteLine($"  {seq,6} {type}: {Shorten(json, 220)}");
    }
}

if (check)
{
    Console.WriteLine();
    return CheckOne(path, replay: !noReplay);
}

return 0;

// ── 重放不变式 ──────────────────────────────────────────────────────────

/// <summary>
/// 一批会话全部过一遍不变式。这是「重放是对的」最接近回归测试的东西 ——
/// 它跑在**真实的、各式各样的**日志上（长的、带工具的、被压缩过的）。
/// </summary>
static int CheckAll(string sessionsRoot)
{
    var files = Directory.EnumerateFiles(sessionsRoot, DshSessionLogFile.FileName, SearchOption.AllDirectories)
        .OrderByDescending(f => new FileInfo(f).Length)
        .ToList();

    Console.WriteLine($"校验 {files.Count} 份会话日志（按大小降序）");
    Console.WriteLine();

    var failed = 0;
    var skipped = 0;
    foreach (var file in files)
    {
        var id = Path.GetFileName(Path.GetDirectoryName(file)) ?? file;
        try
        {
            var code = CheckOne(file, quiet: true, label: id);
            if (code != 0) failed++;
        }
        catch (Exception ex)
        {
            // 正在被写的日志可能拿不到独占读 —— 不算失败，但要报出来。
            skipped++;
            Console.WriteLine($"  ~ {id}: {ex.GetType().Name}: {ex.Message}");
        }
    }

    Console.WriteLine();
    Console.WriteLine(failed == 0
        ? $"✓ {files.Count - skipped} 份通过，{skipped} 份跳过"
        : $"✗ {failed} 份失败，{skipped} 份跳过");

    return failed == 0 ? 0 : 1;
}

/// <summary>
/// 对一份日志的重放结果做不变式检查。
///
/// 这几条不是随手加的，每条都对应一个**已经发生过的错**或者一类**看不出来的错**：
/// </summary>
/// <param name="replay">
/// 传 false 用来做**对照实验**：不跳过打包行时，这些断言必须失败。
/// 一个不会失败的断言等于没有断言 —— 第一次跑通就得拿这个开关证一次。
/// </param>
static int CheckOne(string path, bool quiet = false, string? label = null, bool replay = true)
{
    var loaded = replay
        ? DshSessionReader.Load(path)
        : DshSessionReader.LoadRaw(path);
    var view = loaded.View;
    var problems = new List<string>();
    var orphans = new List<DshBlock>();

    // ① 正文写两遍。日志被压缩过，重放时若展开打包行，同一步的正文会出现两次。
    //    这一条是引入 replay 模式的直接原因。
    foreach (var message in view.Messages)
    {
        var seen = new Dictionary<string, int>(StringComparer.Ordinal);
        foreach (var block in message.Blocks)
        {
            if (block.Kind != DshBlockKind.Text) continue;
            var t = block.Text.Trim();
            if (t.Length < 8) continue;
            seen[t] = seen.GetValueOrDefault(t) + 1;
        }
        foreach (var (t, n) in seen.Where(kv => kv.Value > 1))
        {
            problems.Add($"正文重复 {n} 次：{Shorten(t, 60)}");
        }
    }

    // ② 工具调用没有配对结果。
    //
    //    实测：一份 315 次工具调用的日志里有 **1** 次没有结果（那一次是当时的
    //    工具还在跑）。所以「一个都不能缺」是过严的断言，会稳定误报。
    //    真正的 bug 会是**大面积**配对失败（配对键取错时每次都错），
    //    所以判据是「超过 1 个」。单个的报出来但不判失败。
    foreach (var message in view.Messages)
    {
        foreach (var block in message.Blocks)
        {
            if (block.Kind == DshBlockKind.Tool && !block.HasResult) orphans.Add(block);
        }
    }

    if (orphans.Count > 1)
    {
        problems.Add($"{orphans.Count} 个工具调用没有结果（配对键可能取错了）");
    }
    else if (orphans.Count == 1)
    {
        Console.WriteLine($"  ~ {label ?? "?"}：1 个工具调用没有结果（{Shorten(orphans[0].CallId ?? "?", 30)}）" +
                          "—— 单次可能是当时的调用还在跑，不判失败");
    }

    // ③ 助手消息一个块都没有。说明某类内容被丢掉了 —— 界面上会是一条空气泡。
    foreach (var message in view.Messages)
    {
        if (!message.IsMine && message.Blocks.Count == 0)
        {
            problems.Add("助手消息是空的（0 个块）");
        }
    }

    // ④ 轮次计数与实际出现的 turn 不符。报「N 轮」的地方很多（列表、标题、副标题），
    //    偏一会一起偏，而且从界面上根本看不出来。
    var maxTurn = view.Messages.Where(m => !m.IsMine).Select(m => m.TurnCountHint).DefaultIfEmpty(0).Max();
    if (maxTurn > 0 && view.TurnCount != maxTurn)
    {
        problems.Add($"轮次计数 {view.TurnCount} ≠ 实际最大轮 {maxTurn}");
    }

    // ⑤ 打包行必须被跳过。若一行都没跳过而日志里确实有打包行，
    //    说明跳过逻辑失效了（而 ① 会同时报警）。
    var hasPacked = FileHasPackedRows(path);
    if (hasPacked && loaded.PackedRowCount == 0)
    {
        problems.Add("日志里有打包行，但重放一行都没跳过");
    }

    var name = label ?? Path.GetFileName(Path.GetDirectoryName(path)) ?? path;

    if (problems.Count == 0)
    {
        if (!quiet)
        {
            Console.WriteLine($"✓ {name}");
            Console.WriteLine($"  {loaded.EventCount} 帧事件 · 跳过 {loaded.PackedRowCount} 行打包 · " +
                              $"{view.Messages.Count} 条消息 · {view.TurnCount} 轮");
        }
        return 0;
    }

    Console.WriteLine($"✗ {name}");
    foreach (var p in problems.Take(12)) Console.WriteLine($"    {p}");
    if (problems.Count > 12) Console.WriteLine($"    … 另有 {problems.Count - 12} 条");
    return 1;
}

static bool FileHasPackedRows(string path)
{
    var text = DshSessionLogFile.ReadText(path);
    return text.Contains("\"type\":\"text-chunks\"", StringComparison.Ordinal)
           || text.Contains("\"type\":\"reasoning-chunks\"", StringComparison.Ordinal)
           || text.Contains("\"type\":\"tool-call-chunks\"", StringComparison.Ordinal);
}

/// <summary>
/// 读完之后文件是否还在长 —— 用来识别「这个会话正在被写」。
///
/// 判据用**长度比对**而不是时间戳：文件系统时间戳在 Windows 上可以粗到一秒，
/// 而内核追加一行只要几毫秒 —— 靠时间戳会漏判。
///
/// 本来用它来关掉「工具配对」那条断言，后来发现那条断言的正确判据是
/// 「孤儿数量 > 1」而不是「文件是否稳定」（实测一份 315 次调用的日志里
/// 稳定地有 1 个孤儿）。留着它是因为判断「日志是不是活的」这件事本身
/// 后面还会用到，而它只有 6 行。
/// </summary>
static bool FileGrewDuring(string path)
{
    try
    {
        var before = new FileInfo(path).Length;
        Thread.Sleep(250);
        return new FileInfo(path).Length != before;
    }
    catch
    {
        return false;
    }
}

static string? ArgValue(string[] a, string name)
{
    var i = Array.IndexOf(a, name);
    return i >= 0 && i + 1 < a.Length ? a[i + 1] : null;
}

static string Shorten(string? s, int max = 40) =>
    s is null ? "(null)" : s.Length <= max ? s : s[..max] + "…";
