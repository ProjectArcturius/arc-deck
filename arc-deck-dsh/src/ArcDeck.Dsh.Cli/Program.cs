using System.Text;
using ArcDeck.Dsh;
using ArcDeck.Dsh.Harness;
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

        // ── harness web 这条路（完整功能面）────────────────────────────
        var harnessList = args.Contains("--harness-list");
        var harnessVerify = ArgValue(args, "--harness-verify");
        var harnessPrompt = ArgValue(args, "--harness-prompt");
        var harnessCheckAll = args.Contains("--harness-check-all");

        if (harnessList || harnessCheckAll || harnessVerify is not null || harnessPrompt is not null)
        {
            return await RunHarnessModeAsync(harnessList, harnessCheckAll, harnessVerify, harnessPrompt, cwd).ConfigureAwait(false);
        }

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

    /// <summary>
    /// harness web 这条路 —— 起内核、登录、用 **Remote 层**（完整功能面）。
    ///
    /// 与 SDK 那条路的区别见 <see cref="ArcDeck.Dsh.Harness.HarnessHost"/> 的类注释：
    /// 这条路能列会话、读历史、改名、取消，因为它是 WebView 客户端用的那套 API。
    /// </summary>
    private static async Task<int> RunHarnessModeAsync(bool list, bool checkAll, string? verify, string? prompt, string cwd)
    {
        Console.WriteLine("Arc Deck · harness Remote 层（完整功能面）");
        Console.WriteLine("──────────────────────────────────────────────");

        var install = HarnessLocator.Locate();
        if (install is null)
        {
            Console.WriteLine("✗ 找不到能跑 `dsh web` 的安装（要求 web profile 已装）。");
            Console.WriteLine("  找过：ARC_DECK_DSH_ROOT、%LOCALAPPDATA%\\Programs\\DSH Desktop、");
            Console.WriteLine("        %ProgramFiles%\\DSH Desktop；profile 要落在 <DSH_HOME>\\profiles\\web。");
            return 2;
        }

        Console.WriteLine("✓ 找到安装:");
        Console.WriteLine("  " + install);
        Console.WriteLine();
        Console.WriteLine("启动 harness web（--no-open，不会去拉起默认浏览器）…");

        var started = DateTimeOffset.Now;
        await using var host = await HarnessHost.StartAsync(install, diagnostic: m => Console.WriteLine("  " + m));
        if (host is null)
        {
            Console.WriteLine("✗ 内核没起来或没登录成功");
            return 2;
        }

        Console.WriteLine($"✓ 已登录（{DateTimeOffset.Now - started:hh\\:mm\\:ss\\.fff}）");
        await using var client = await HarnessSessionClient.ConnectAsync(host);

        var sessions = await client.ListSessionsAsync();
        Console.WriteLine($"✓ session/list：{sessions.Count} 个会话" +
                          $"（{sessions.Count(s => s.Running)} 个运行中，" +
                          $"{sessions.Count(s => s.IsSubagent)} 个子 agent）");

        if (list || (!checkAll && verify is null && prompt is null))
        {
            Console.WriteLine();
            foreach (var s in sessions.Where(s => !s.IsSubagent).Take(15))
            {
                Console.WriteLine($"  {(s.Running ? "▶" : " ")} {s.SessionId}  {s.UpdatedAt:MM-dd HH:mm}  {s.Subtitle}");
            }
            return 0;
        }

        if (checkAll)
        {
            return await CheckAllHarnessSessionsAsync(client, install, sessions).ConfigureAwait(false);
        }

        // 目标会话：显式给的那个，否则第一个「有内容、不是子 agent」的。
        var target = verify is not null
            ? sessions.FirstOrDefault(s => s.SessionId == verify)
            : sessions.FirstOrDefault(s => !s.IsSubagent && !s.Blank);

        if (target is null)
        {
            Console.WriteLine($"✗ 没有可用会话{(verify is not null ? $"（找不到 {verify}）" : "")}");
            return 1;
        }

        Console.WriteLine();
        Console.WriteLine($"打开会话 {target.SessionId}（session/follow）…");

        HarnessSessionStream session;
        try
        {
            session = await client.OpenAsync(target.SessionId, withAssistantStream: false,
                loadAllHistory: true);
        }
        catch (HarnessRpcException ex)
        {
            // **这条错误要原样给人看。** 实测内核会报
            // 「corrupt Zstandard session log: complete frame contains a torn JSONL record」
            // —— 那是一个正在被写的大会话，重试或换一个会话就好。
            // 界面上必须显示成「这个会话现在打不开」，而不是空白页。
            Console.WriteLine($"✗ 打不开：{ex.Message}");
            return 1;
        }

        Console.WriteLine($"✓ 快照 {session.PagesLoaded} 页 / {session.SnapshotRecords} 条记录 · " +
                          $"折叠 {session.FoldedEvents} 帧事件 · 跳过 {session.SkippedPackedRows} 行打包 · " +
                          $"cursor={session.Cursor} · 历史取全={session.HistoryComplete}");

        if (prompt is not null)
        {
            var accepted = await client.PromptAsync(target.SessionId, prompt);
            Console.WriteLine($"→ prompt 回执 accepted={accepted}（只是入队）");
            Console.WriteLine("等 live 事件（最多 60s，收到 turn/end 就停）…");

            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(60));
            var histogram = await client.PumpAsync(session, (change, evt) =>
            {
                if (change.HasFlag(DshProjection.Change.Structure))
                    Console.WriteLine($"  [结构] {evt.Type} → {session.View.Messages.Count} 条消息");
            }, cts.Token);

            Console.WriteLine("live 事件：" + (histogram.Count == 0
                ? "（一个都没收到）"
                : string.Join(", ", histogram.Select(kv => $"{kv.Key}×{kv.Value}"))));
        }

        Console.WriteLine();
        Console.WriteLine("── follow 出来的投影 ──────────────────────────");
        Console.WriteLine($"会话投影：{session.View.Title}（{session.View.Subtitle}）");
        Console.WriteLine($"模型：{session.View.ModelLabel ?? "（未见 request/header）"}");
        Console.WriteLine("──────────────────────────────────────────────");
        PrintMessage(session.View);

        // ── 对账：Remote 层喂来的事件 vs 磁盘上的事实 ──────────────────
        //
        // 这是 web 这条路**唯一**能证明「线上拿到的和存下来的一致」的手段，
        // 与 SDK 那条路的 `--verify-reload` 是同一个思路，但更硬：
        // 它对的是**两个不同的数据来源**（内核 API 与日志文件），
        // 而 SDK 那条对的是同一条会话的两个时刻。
        if (verify is not null)
        {
            Console.WriteLine();
            Console.WriteLine("── Remote 层 vs 日志重放 ─────────────────────");
            // 现在已经往回翻到底（`loadAllHistory: true`），所以正常情况下这条不会出现。
            if (session.HasMore)
            {
                Console.WriteLine("! 这个会话还有更早的历史没取回（翻页被上限截住了）：");
                Console.WriteLine("  下面的比对**只覆盖已取回的部分**，差异里可能全是「没翻到的部分」。");
            }

            var logInfo = DshSessionReader.Enumerate(install.HarnessHome)
                .FirstOrDefault(s => s.SessionId == target.SessionId);

            if (logInfo is null)
            {
                Console.WriteLine($"! 磁盘上找不到这个会话的日志 —— 只能验证到这一步");
                return 0;
            }

            var replayed = DshSessionReader.Load(logInfo.Path);
            var comparison = DshSessionReader.Compare(session.View, replayed.View);
            Console.WriteLine($"重放：{replayed.EventCount} 帧事件 · 跳过 {replayed.PackedRowCount} 行打包 · " +
                              $"{replayed.View.Messages.Count} 条消息");

            // 两边的「跳过打包」计数**不可直接相减**：日志侧数的是文件里的行，
            // 线上侧数的是内核重新打包后的记录。把线上侧的构成打出来，
            // 差值才有得看 —— 实测同一会话是 4（线上）对 10（日志）。
            Console.WriteLine("线上快照的记录构成：" + string.Join("、",
                session.SnapshotRecordTypes.OrderByDescending(kv => kv.Value)
                    .Select(kv => $"{kv.Key}×{kv.Value}")));

            foreach (var line in comparison.Differences) Console.WriteLine("  " + line);
            Console.WriteLine(comparison.IsEquivalent
                ? "✓ 两条路一致"
                : "✗ 两条路不一致（上面是差异）");
            return comparison.IsEquivalent ? 0 : 1;
        }

        return 0;
    }

    /// <summary>
    /// 对**每一个**真实会话跑一遍「follow 出来的投影 vs 日志重放」。
    ///
    /// 这是 web 这条路最接近回归测试的东西，也正是 SDK 那条路 `--check-all` 的对应物。
    /// 为什么不能只验一个：卡住的那个 bug（`follow` 收下快照却没跳出循环）**只在一个
    /// 不再有人说话的会话上显形** —— 每个已落盘的会话都是这种会话，所以拿一个验过
    /// 已经说明了很多；但「很多」不等于「全部」，而这台机器上有 71 份日志，
    /// 跑完的代价只有几分钟。
    ///
    /// 已知的**不是缺陷**的失败也要分开报：正在被写的会话可能落着半行
    /// （参 P7），内核会以 `corrupt Zstandard session log` 拒开。那类失败要计进
    /// 「打不开」而不是「不一致」—— 混在一起会让「不一致 = 0」这句话没有意义。
    /// </summary>
    private static async Task<int> CheckAllHarnessSessionsAsync(
        HarnessSessionClient client,
        ArcDeck.Dsh.Harness.HarnessInstall install,
        IReadOnlyList<ArcDeck.Dsh.Harness.HarnessSessionInfo> sessions)
    {
        // 子 agent 会话与空会话没有可看的正文，但它们**照样要能打开** ——
        // 所以不排除，只是不指望它们有内容。
        var targets = sessions.ToList();

        Console.WriteLine();
        Console.WriteLine($"── 对 {targets.Count} 个会话逐一对账 ──────────────");

        var equivalent = 0;
        var mismatched = new List<string>();
        var unopenable = new List<string>();
        var paginated = 0;
        var subagents = 0;
        var noLog = 0;
        var totalPages = 0;
        var maxPages = 0;

        foreach (var s in targets)
        {
            // **子 agent 会话要单独算。** 它们的地址不是 `{kind:'session', sessionId}` ——
            // 内核明确拒绝：「subagent Sessions require their durable parent address」。
            // 拿错的地址去试，会得到 44 条一模一样的「打不开」，把真正的问题埋掉。
            // 这是**我们没实现**的能力（父地址形状尚未处理），不是会话坏了 ——
            // 所以不试、也不计进失败（参铁律 2：不假装做了）。
            if (s.IsSubagent)
            {
                subagents++;
                Console.Write("s");
                continue;
            }

            // 每个会话一个**独立的**短上限：这个循环里出现的挂死是我们要抓的
            // 那类 bug 本身，不该让一个坏会话把整轮拖成几分钟。
            HarnessSessionStream stream;
            try
            {
                stream = await client.OpenAsync(s.SessionId, withAssistantStream: false,
                    openBudget: TimeSpan.FromSeconds(60), loadAllHistory: true);
            }
            catch (HarnessRpcException ex)
            {
                unopenable.Add($"{s.SessionId}  [{ex.Code}] {FirstLine(ex.Message)}");
                Console.Write("✗");
                continue;
            }

            // **`hasMore` 必须看。** 快照是**一页**，不是全部历史。现在
            // `loadAllHistory: true` 会往回翻到底，所以正常情况下这里不该再为 true；
            // 真为 true 只可能是翻页被上限截住了 —— 那时如实报「只比了一页」，
            // 而不是把它算成不一致（参 P13）。
            if (stream.HasMore)
            {
                paginated++;
                Console.Write("…");
                continue;
            }

            var logInfo = DshSessionReader.Enumerate(install.HarnessHome)
                .FirstOrDefault(x => x.SessionId == s.SessionId);
            if (logInfo is null)
            {
                noLog++;
                Console.Write("?");
                continue;
            }

            var replayed = DshSessionReader.Load(logInfo.Path);
            var comparison = DshSessionReader.Compare(stream.View, replayed.View);

            // 把「翻了多少页」也累计起来 —— 否则「一致 25」看不出往回翻到底有没有真的发生：
            // 若 hasMore 恰好全是 false，25 这个数字与「只翻了 1 页」是一模一样的。
            totalPages += stream.PagesLoaded;
            maxPages = Math.Max(maxPages, stream.PagesLoaded);

            if (comparison.IsEquivalent)
            {
                equivalent++;
                Console.Write("·");
            }
            else
            {
                mismatched.Add($"{s.SessionId}（线上 {stream.SnapshotRecords} 条记录 / " +
                               $"{stream.View.Messages.Count} 条消息，重放 {replayed.EventCount} 帧 / " +
                               $"{replayed.View.Messages.Count} 条消息）\n      " +
                               string.Join("\n      ", comparison.Differences.Take(5)));
                Console.Write("✗");
            }
        }

        Console.WriteLine();
        Console.WriteLine();
        Console.WriteLine($"一致 {equivalent} · 不一致 {mismatched.Count} · " +
                          $"只比了一页 {paginated} · 子 agent 跳过 {subagents} · " +
                          $"打不开 {unopenable.Count} · 磁盘上没有日志 {noLog}");
        Console.WriteLine($"往回翻了 {totalPages} 页（最多的一个会话 {maxPages} 页）" +
                          $"—— 1 页意味着那条会话的快照本来就装下了全部历史");

        if (mismatched.Count > 0)
        {
            Console.WriteLine();
            Console.WriteLine("── 不一致的会话 ──────────────────────────────");
            foreach (var line in mismatched) Console.WriteLine("  " + line);
        }

        if (unopenable.Count > 0)
        {
            Console.WriteLine();
            Console.WriteLine("── 打不开的会话（内核自己报的日志损坏）─────────");
            foreach (var line in unopenable) Console.WriteLine("  " + line);
        }

        if (subagents > 0)
        {
            Console.WriteLine();
            Console.WriteLine($"  （另有 {subagents} 个子 agent 会话未尝试：它们的地址要带父地址，" +
                              "客户端还没实现 —— 这是缺的功能，不是会话的问题）");
        }

        // 只要有一条**内容不一致**就不算通过。「打不开」同样让命令失败 ——
        // 但它在上面被单独列出来了，读的人能分清是哪一种。
        if (mismatched.Count == 0 && unopenable.Count == 0)
        {
            Console.WriteLine();
            Console.WriteLine("✓ 全部会话：线上投影与日志重放一致");
            return 0;
        }

        return 1;
    }

    private static string FirstLine(string s)
    {
        var i = s.IndexOf('\n');
        return i < 0 ? s : s[..i] + "…";
    }

    /// <summary>把一个会话的全部消息打出来（CLI 侧的统一打印，实时与重放共用）。</summary>
    private static void PrintMessage(DshSessionView view)
    {
        foreach (var message in view.Messages) PrintMessage(message);
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
