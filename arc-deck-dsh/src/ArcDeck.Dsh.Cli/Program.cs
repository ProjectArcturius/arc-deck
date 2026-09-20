using System.Text;
using ArcDeck.Dsh;
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

        var messageId = await client.SendAsync(sessionId, prompt);
        Console.WriteLine($"← 回执 messageId={messageId}（只表示已入队）");

        // 等 idle。协议没有「这一轮结束」的方法，只能等状态机回到 idle ——
        // 上游文档明确说明这一点，所以这里等的是状态，不是某个消息。
        var deadline = DateTimeOffset.Now.AddMinutes(3);
        while (view.IsRunning && DateTimeOffset.Now < deadline)
        {
            await Task.Delay(200);
        }

        Console.WriteLine();
        Console.WriteLine("══════════════════════════════════════════════");
        Console.WriteLine($"会话投影：{view.Title}（{view.Subtitle}）");
        Console.WriteLine($"模型：{view.ModelLabel ?? "（未见 request/header）"}");
        Console.WriteLine("──────────────────────────────────────────────");

        foreach (var message in view.Messages)
        {
            Console.WriteLine();
            Console.WriteLine($"[{(message.IsMine ? "我" : "内核")}] {message.Meta}");
            foreach (var block in message.Blocks)
            {
                switch (block.Kind)
                {
                    case DshBlockKind.Text:
                        Console.WriteLine(Indent(block.Text));
                        break;
                    case DshBlockKind.Tool:
                        Console.WriteLine($"  ⌨ {block.ToolName} {Truncate(block.ToolArguments, 120)}");
                        Console.WriteLine($"    结果{(block.HasResult ? "" : "（未回）")}: {Truncate(block.Text, 400)}");
                        break;
                    default:
                        Console.WriteLine($"  ? [{block.SourceType}] {Truncate(block.Text, 400)}");
                        break;
                }
            }
        }

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

        // 退出码表达「助手有没有真的回话」—— 只报「跑完了」的验收工具
        // 会在模型不可用时给一个绿灯，那是最坏的一种工具。
        var gotReply = view.Messages.Any(m => !m.IsMine && m.Blocks.Any(b => b.Kind == DshBlockKind.Text));
        Console.WriteLine(gotReply ? "✓ 助手产出了正文" : "✗ 助手没有产出任何正文（见上面的失败块与事件类型）");
        Console.WriteLine("投影结束。");

        return gotReply ? 0 : 1;
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
