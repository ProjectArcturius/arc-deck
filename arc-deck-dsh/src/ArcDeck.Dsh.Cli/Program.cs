using System.Text;
using ArcDeck.Dsh;
using ArcDeck.Dsh.Runtime;
using ArcDeck.Dsh.Sessions;
using ArcDeck.Dsh.View;

namespace ArcDeck.Dsh.Cli;

/// <summary>
/// 协议层的验收工具 —— 一层没有界面也能跑通的证明。
///
/// 用它的场合：改了协议层之后，先在这里看帧与投影对不对，再接界面。
/// 反过来（先接界面再调协议）会让每一次失败都变成两处嫌疑。
///
/// 用法：
///   dsh-cli                       # 找内核、起会话、发一条固定提示词、打印投影
///   dsh-cli --prompt "…"          # 换提示词
///   dsh-cli --cwd &lt;目录&gt;          # 换工作区
///   dsh-cli --locate-only         # 只报告「内核在哪」，不起进程
/// </summary>
internal static class Program
{
    private static async Task<int> Main(string[] args)
    {
        Console.OutputEncoding = Encoding.UTF8;

        var prompt = ArgValue(args, "--prompt") ?? "只回复两个字：收到";
        var cwd = ArgValue(args, "--cwd") ?? Directory.GetCurrentDirectory();
        var locateOnly = args.Contains("--locate-only");
        var quiet = args.Contains("--quiet");
        var dumpEvents = args.Contains("--dump-events");
        var trace = args.Contains("--trace");
        var list = args.Contains("--list");
        var load = ArgValue(args, "--load");
        var verifyReload = args.Contains("--verify-reload");

        Console.WriteLine("Arc Deck · DSH 协议客户端");
        Console.WriteLine("──────────────────────────────────────────────");

        var layout = DshRuntimeLocator.Locate();
        if (layout is null)
        {
            Console.WriteLine("✗ 找不到 dsh 安装。");
            Console.WriteLine("  找过：ARC_DECK_DSH_ROOT、%LOCALAPPDATA%\\Programs\\DSH Desktop、");
            Console.WriteLine("        %ProgramFiles%\\DSH Desktop、以及 PATH 上的 node。");
            Console.WriteLine("  设 ARC_DECK_DSH_ROOT 指向安装目录（含 resources\\app）可以显式指定。");
            return 2;
        }

        Console.WriteLine("✓ 找到内核:");
        Console.WriteLine("  " + layout);
        Console.WriteLine();
        Console.WriteLine($"预期上游版本: {DshVersion.PinnedRuntimeVersion}（协议无版本协商，只能钉）");
        Console.WriteLine($"工作区: {cwd}");

        if (locateOnly) return 0;

        // ── 只读路径：列会话 / 重载会话 ──────────────────────────────
        // 这两条**不启动内核**。重载走的是磁盘上的日志，与内核无关 ——
        // 让一个只需要读文件的操作去起 200 MB 的 Node 进程是没道理的，
        // 而且那样一来「重载对不对」就同时取决于两件事。
        if (list || load is not null)
        {
            return list ? ListSessions(layout, cwd) : LoadSession(layout, load!);
        }

        Console.WriteLine();
        Console.WriteLine("启动内核并握手（首次加载插件树可能要十几秒）…");

        var started = DateTimeOffset.Now;
        var log = new List<string>();

        await using var client = await DshClient.ConnectAsync(cwd, m =>
        {
            log.Add(m);
            if (!quiet) Console.WriteLine("  " + m);
        });

        if (client is null)
        {
            Console.WriteLine("✗ 内核启动失败");
            return 2;
        }

        Console.WriteLine($"✓ 握手完成（{DateTimeOffset.Now - started:hh\\:mm\\:ss\\.fff}）");

        var sessionId = $"arc-{DateTimeOffset.UtcNow:yyyyMMddHHmmss}-{Guid.NewGuid():N}"[..40];
        var view = client.OpenSession(sessionId, cwd, "协议验收");
        var projection = client.ProjectionOf(sessionId);

        var reported = 0;
        if (trace)
        {
            // 逐帧原样打出来。投影的问题十次有九次是「事件到达的顺序或形状
            // 与设想不同」，而这件事只能靠看真实序列来确认 —— 猜没有用。
            client.SessionChanged += (_, e) =>
            {
                var turn = e.Event.Data?["turn"]?.GetValue<int>();
                var step = e.Event.Data?["step"]?.GetValue<int>();
                var index = e.Event.Data?["chunk"]?["index"]?.GetValue<int>();
                var chunkType = e.Event.Data?["chunk"]?["type"]?.GetValue<string>();
                Console.WriteLine(
                    $"[帧] {e.Event.Sequence,3} {e.Event.Type,-20} turn={turn} step={step} " +
                    $"idx={index} chunk={chunkType} → {e.Change}");
            };
        }

        client.SessionChanged += (_, e) =>
        {
            if (e.View.SessionId != sessionId) return;

            if (e.Change.HasFlag(DshProjection.Change.Structure))
            {
                Console.WriteLine($"\n[结构] {e.Event.Type} → 现在 {e.View.Messages.Count} 条消息");
            }
            else if (e.Change.HasFlag(DshProjection.Change.Text))
            {
                var total = TotalText(view);
                // 流式正文按量刷，不逐 token 打 —— 那样控制台会被刷爆，
                // 反而看不出哪一步在动。
                if (total - reported > 40)
                {
                    reported = total;
                    Console.WriteLine($"  … 正文 {total} 字");
                }
            }
        };

        client.StatusChanged += (_, e) =>
        {
            if (e.SessionId != sessionId) return;
            Console.WriteLine($"[状态] {(e.IsRunning ? "running" : "idle")}");
        };

        Console.WriteLine();
        Console.WriteLine($"→ session/prompt: {prompt}");
        Console.WriteLine($"  会话 id：{sessionId}");

        var messageId = await client.SendAsync(sessionId, prompt);
        Console.WriteLine($"← 回执 messageId={messageId}（只表示已入队）");

        // 等 idle。协议没有「这一轮结束」的方法，只能等状态机回到 idle ——
        // 上游文档明确说明这一点，所以这里等的是状态，不是某个消息。
        var deadline = DateTimeOffset.Now.AddMinutes(3);
        while (view.IsRunning && DateTimeOffset.Now < deadline)
        {
            await Task.Delay(200);
        }

        // 把会话 id 打在最显眼的位置：**重载的关键证据是「同一条会话」
        // 在实时与重放两条路径下投影一致**，而重放要靠这个 id 去找日志。
        // 不给它，就只能靠时间去猜是哪一份。
        Console.WriteLine();
        Console.WriteLine($"会话 id 留档：{sessionId}");
        Console.WriteLine("  重放同一份日志（新起一个进程、只读磁盘、不碰内核）：");
        Console.WriteLine($"    arc-deck-dsh/tools/log-probe --check 时把上面这个 id 传给 dsh-cli --load {sessionId}");

        Console.WriteLine();
        Console.WriteLine("══════════════════════════════════════════════");
        Console.WriteLine($"会话投影：{view.Title}（{view.Subtitle}）");
        Console.WriteLine($"模型：{view.ModelLabel ?? "（未见 request/header）"}");
        Console.WriteLine("──────────────────────────────────────────────");

        foreach (var message in view.Messages) PrintMessage(message);

        Console.WriteLine();
        Console.WriteLine("── 统计 ──────────────────────────────────────");
        Console.WriteLine($"消息 {view.Messages.Count} 条 · 轮次 {view.TurnCount}");
        Console.WriteLine($"诊断 {log.Count} 行（内核 stderr 与非协议输出）");
        Console.WriteLine($"见过的事件类型 {projection.RecordedTypes.Count} 种：" +
                          string.Join(", ", projection.RecordedTypes.OrderBy(x => x, StringComparer.Ordinal)));

        if (dumpEvents)
        {
            Console.WriteLine();
            Console.WriteLine("── 内核内部事件（界面不画，原文） ─────────────");
            foreach (var block in projection.Recorded)
            {
                Console.WriteLine($"  {block.SourceType}: {Truncate(block.Text, 500)}");
            }
        }

        // ── 实时 vs 重放：同一条会话、两条路径、逐块比对 ────────────────
        //
        // 这是本轮最有价值的一条证据：它证明「界面上刚才看到的」与
        // 「下次打开时会看到的」是**同一份东西**。要等进程关掉、内核把日志
        // flush 完之后再读，否则会比一份正在写的日志。
        if (verifyReload)
        {
            Console.WriteLine();
            Console.WriteLine("── 实时 vs 重放 ──────────────────────────────");
            Console.WriteLine("（先关内核让日志落盘，再用只读路径重放同一份日志）");

            await client.DisposeAsync();

            var hit = DshSessionReader.Enumerate(layout.HarnessHome)
                .FirstOrDefault(s => s.SessionId == sessionId);

            if (hit is null)
            {
                Console.WriteLine($"✗ 磁盘上找不到这个会话（{sessionId}）—— 日志没落盘？");
                return 1;
            }

            var reloaded = DshSessionReader.Load(hit.Path);
            var comparison = DshSessionReader.Compare(view, reloaded.View);
            Console.WriteLine($"重放：{reloaded.EventCount} 帧事件 · 跳过 {reloaded.PackedRowCount} 行打包 · " +
                              $"{reloaded.View.Messages.Count} 条消息");
            foreach (var line in comparison.Differences) Console.WriteLine("  " + line);
            Console.WriteLine(comparison.IsEquivalent
                ? "✓ 两条路径一致"
                : "✗ 两条路径不一致（上面是差异）");
            return comparison.IsEquivalent ? 0 : 1;
        }

        // 退出码表达「助手有没有真的回话」—— 只报「跑完了」的验收工具
        // 会在模型不可用时给一个绿灯，那是最坏的一种工具。
        var gotReply = view.Messages.Any(m => !m.IsMine && m.Blocks.Any(b => b.Kind == DshBlockKind.Text));
        Console.WriteLine(gotReply ? "✓ 助手产出了正文" : "✗ 助手没有产出任何正文（见上面的失败块与事件类型）");
        Console.WriteLine("投影结束。");

        return gotReply ? 0 : 1;
    }

    /// <summary>
    /// 列磁盘上的会话。
    ///
    /// 只读会话头，不解压全文 —— 列表页要的是「有哪几个会话、各多大、什么时候动过」，
    /// 不需要正文。所以这一条很快，也不启动内核。
    /// </summary>
    private static int ListSessions(DshRuntimeLayout layout, string cwd)
    {
        var all = DshSessionReader.Enumerate(layout.HarnessHome);
        var here = DshSessionReader.Enumerate(layout.HarnessHome, cwd);

        Console.WriteLine();
        Console.WriteLine($"会话日志根目录: {DshSessionReader.SessionsRoot(layout.HarnessHome)}");
        Console.WriteLine($"全部 {all.Count} 个 · 属于当前工作区 {here.Count} 个");
        Console.WriteLine("──────────────────────────────────────────────");

        foreach (var s in all)
        {
            var mine = here.Any(h => h.SessionId == s.SessionId) ? "●" : " ";
            Console.WriteLine(
                $"{mine} {s.SessionId}");
            Console.WriteLine(
                $"    {s.ModifiedAt:yyyy-MM-dd HH:mm}  {s.CompressedBytes / 1024.0,7:F0} KB  {s.Workspace}");
        }

        Console.WriteLine();
        Console.WriteLine("（● = 属于当前工作区；标题要重载才知道，列表这一步刻意不解压全文）");
        return 0;
    }

    /// <summary>
    /// 从磁盘重载一个会话并打印它。
    ///
    /// 这是「重载出来的和当时看到的一致」这句话的**唯一**证据来源 ——
    /// 它走的是与实时完全相同的折叠路径（<see cref="DshProjection"/>），
    /// 只是事件来源从 stdio 换成了日志文件。
    /// </summary>
    private static int LoadSession(DshRuntimeLayout layout, string idOrPath)
    {
        string path;
        if (File.Exists(idOrPath))
        {
            path = idOrPath;
        }
        else
        {
            var hit = DshSessionReader.Enumerate(layout.HarnessHome)
                .FirstOrDefault(s => s.SessionId == idOrPath
                                     || s.SessionId.StartsWith(idOrPath, StringComparison.OrdinalIgnoreCase));
            if (hit is null)
            {
                Console.WriteLine($"✗ 找不到会话「{idOrPath}」。用 --list 看有哪些。");
                return 2;
            }
            path = hit.Path;
        }

        Console.WriteLine();
        Console.WriteLine($"重载 {path}");

        var started = DateTimeOffset.Now;
        var loaded = DshSessionReader.Load(path);
        var elapsed = DateTimeOffset.Now - started;

        var view = loaded.View;
        Console.WriteLine($"✓ {loaded.EventCount} 帧事件，{elapsed.TotalMilliseconds:F0} ms" +
                          (loaded.TailTruncated ? "（末尾有半行 —— 内核可能正在写这个会话）" : ""));
        Console.WriteLine();
        Console.WriteLine("══════════════════════════════════════════════");
        Console.WriteLine($"会话投影：{view.Title}（{view.Subtitle}）");
        Console.WriteLine($"会话 id：{view.SessionId}");
        Console.WriteLine($"工作区：{loaded.Header.Workspace}");
        Console.WriteLine($"创建于：{loaded.Header.CreatedAt:yyyy-MM-dd HH:mm}  预设：{loaded.Header.AgentPreset}");
        Console.WriteLine($"模型：{view.ModelLabel ?? "（未见 request/header）"}");
        Console.WriteLine("──────────────────────────────────────────────");

        foreach (var message in view.Messages) PrintMessage(message);

        Console.WriteLine();
        Console.WriteLine("── 统计 ──────────────────────────────────────");
        Console.WriteLine($"消息 {view.Messages.Count} 条 · 轮次 {view.TurnCount}");
        Console.WriteLine($"重载 {loaded.EventCount} 帧事件 · 跳过 {loaded.PackedRowCount} 行打包编码");
        Console.WriteLine($"见过的事件类型 {loaded.UnhandledTypes.Count} 种：" +
                          string.Join(", ", loaded.UnhandledTypes.OrderBy(x => x, StringComparer.Ordinal)));

        return view.Messages.Count > 0 ? 0 : 1;
    }

    /// <summary>
    /// 把一个块打印成人读得懂的一行/一段。
    ///
    /// 实时路径与重载路径共用这一份 —— 两条路径的差别只该在**事件来源**上，
    /// 打印方式分叉会让「看到的和存下来的一致」这个判断失去意义
    /// （你会分不清差异来自投影还是来自打印）。
    /// </summary>
    private static void PrintBlock(DshBlock block, string indent)
    {
        switch (block.Kind)
        {
            case DshBlockKind.Text:
                Console.WriteLine(Indent(block.Text));
                break;

            case DshBlockKind.Reasoning:
                Console.WriteLine($"{indent}💭 思考 {block.Text.Length} 字（默认折叠）：{Truncate(block.Text, 160)}");
                break;

            case DshBlockKind.Tool:
                var state = block.IsError ? "✗" : block.HasResult ? "✓" : "…";
                Console.WriteLine($"{indent}{state} ⌨ {block.ToolName}  {Truncate(block.Title, 100)}");
                if (block.HasResult)
                {
                    Console.WriteLine($"{indent}    结果 {block.Text.Length} 字：{Truncate(block.Text, 300)}");
                }
                else
                {
                    Console.WriteLine($"{indent}    结果未回");
                }
                break;

            case DshBlockKind.Raw:
                Console.WriteLine($"{indent}· [{block.SourceType}] {Truncate(block.Text, 300)}");
                break;

            default:
                Console.WriteLine($"{indent}? [{block.SourceType}] {Truncate(block.Text, 300)}");
                break;
        }
    }

    private static void PrintMessage(DshMessageView message)
    {
        Console.WriteLine();
        Console.WriteLine($"[{(message.IsMine ? "我" : "内核")}] {message.Meta}");

        // 一轮里工具卡可能有几十上百个。全打出来会把终端刷爆，
        // 所以正文与思考全打、工具卡只打前若干个，剩下的折叠成一行计数 ——
        // 这与界面上「默认折叠」的取舍是同一件事。
        const int MaxToolCards = 8;
        var tools = message.Blocks.Count(b => b.Kind == DshBlockKind.Tool);
        var printed = 0;

        foreach (var block in message.Blocks)
        {
            if (block.Kind == DshBlockKind.Tool)
            {
                if (printed >= MaxToolCards)
                {
                    continue;
                }
                printed++;
            }
            PrintBlock(block, "  ");
        }

        if (tools > MaxToolCards)
        {
            Console.WriteLine($"  … 另有 {tools - MaxToolCards} 个工具调用（界面里是可展开的卡片）");
        }
    }

    private static int TotalText(DshSessionView view)
    {
        var n = 0;
        foreach (var m in view.Messages)
            foreach (var b in m.Blocks)
                n += b.Text.Length;
        return n;
    }

    private static string Indent(string text) =>
        string.Join('\n', text.Split('\n').Select(l => "  " + l));

    private static string Truncate(string? s, int max)
    {
        if (string.IsNullOrEmpty(s)) return "(空)";
        s = s.Replace('\n', ' ');
        return s.Length <= max ? s : s[..max] + "…";
    }

    private static string? ArgValue(string[] args, string name)
    {
        var i = Array.IndexOf(args, name);
        return i >= 0 && i + 1 < args.Length ? args[i + 1] : null;
    }
}
