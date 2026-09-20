using System.Diagnostics;
using System.Text.Json.Nodes;
using ArcDeck.Dsh.Transport;

namespace ArcDeck.Dsh.Runtime;

/// <summary>
/// 一个 dsh 运行时进程 —— 起它、跟它说 SDK 协议、关掉它。
///
/// 这一层刻意**不做**事件翻译：翻译在 <see cref="DshClient"/>。
/// 分开的理由是可测性 —— 翻译逻辑（哪一种事件产生哪一个块）是最容易出错、
/// 也最需要用真实会话日志反复回放的部分，它不该需要真的起一个内核才能跑。
/// </summary>
public sealed class DshRuntime : IAsyncDisposable
{
    private readonly DshRuntimeLayout _layout;
    private Process? _process;
    private JsonRpcLineTransport? _transport;

    /// <summary>内核 stderr 与本地诊断。默认丢弃 —— 订阅方负责落到日志。</summary>
    public event Action<string>? Diagnostic;

    /// <summary>服务端通知（<c>session.event</c> / <c>session.status</c> / <c>subagent.*</c>）。</summary>
    public event Action<string, JsonNode?>? NotificationReceived;

    public DshRuntimeLayout Layout => _layout;
    public bool IsRunning => _process is { HasExited: false };

    private DshRuntime(DshRuntimeLayout layout) => _layout = layout;

    public static async Task<DshRuntime?> StartAsync(
        string? harnessHome = null,
        Action<string>? diagnostic = null,
        CancellationToken cancellationToken = default)
    {
        var layout = DshRuntimeLocator.Locate(harnessHome);
        if (layout is null) return null;

        var runtime = new DshRuntime(layout);
        if (diagnostic is not null) runtime.Diagnostic += diagnostic;

        await runtime.SpawnAsync(cancellationToken).ConfigureAwait(false);
        return runtime;
    }

    private async Task SpawnAsync(CancellationToken cancellationToken)
    {
        SdkProfileProvisioner.Ensure(_layout.ProfileDirectory);

        var psi = new ProcessStartInfo(_layout.NodeExecutable)
        {
            // 只挂 sdk profile。不加 --patch：DSH Desktop 那份 patch 指向 web 应用
            // （WebSocket 服务与前端），挂到 sdk profile 上会让内核起不来。
            Arguments = $"\"{_layout.DshEntry}\" --profile sdk",
            WorkingDirectory = _layout.ProfileDirectory,
            UseShellExecute = false,          // 必须：否则拿不到 stdin/stdout
            CreateNoWindow = true,
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            StandardOutputEncoding = new System.Text.UTF8Encoding(false),
            StandardErrorEncoding = new System.Text.UTF8Encoding(false),
        };

        // 内核的 home 决定它读哪一份凭据与 settings。不设这个变量会落到默认
        // ~/.dsh，于是「模型适配器起来了但请求失败」，而那条报错看起来像网络问题。
        psi.Environment["DSH_HOME"] = _layout.HarnessHome;

        _process = Process.Start(psi)
            ?? throw new InvalidOperationException("无法启动 dsh 进程");

        // 内核的 stderr 是**唯一**能在「没消息」时说明原因的一条线。
        // 上游在 stdout 上只放 JSON-RPC 帧，任何诊断都在 stderr。
        _ = Task.Run(async () =>
        {
            try
            {
                string? line;
                while ((line = await _process!.StandardError.ReadLineAsync(cancellationToken).ConfigureAwait(false)) is not null)
                    Diagnostic?.Invoke($"[dsh stderr] {line}");
            }
            catch (OperationCanceledException) { }
            catch { }
        }, cancellationToken);

        _transport = new JsonRpcLineTransport(_process.StandardOutput.BaseStream, _process.StandardInput.BaseStream);
        _transport.Diagnostic += m => Diagnostic?.Invoke(m);
        _transport.NotificationReceived += (m, p) => NotificationReceived?.Invoke(m, p);
        _transport.Start();

        await Task.CompletedTask;
    }

    /// <summary>
    /// 握手。
    ///
    /// <c>cwd</c> 是**工作区**：内核用它算 agent 的工作目录，也用它决定
    /// 会话日志落在哪个工作区分组下。传错了消息会写进另一个工作区的会话里，
    /// 而界面看起来完全正常。
    /// </summary>
    public Task<JsonNode?> InitializeAsync(
        string cwd,
        string provider = DshVersion.DefaultProvider,
        string model = DshVersion.DefaultModel,
        string? reasoningEffort = null,
        int? maxTokens = null,
        CancellationToken cancellationToken = default)
    {
        var p = new JsonObject
        {
            ["cwd"] = cwd,
            ["provider"] = provider,
            ["model"] = model,
        };
        if (!string.IsNullOrWhiteSpace(reasoningEffort)) p["reasoningEffort"] = reasoningEffort;
        if (maxTokens is > 0) p["maxTokens"] = maxTokens;

        return Transport.RequestAsync("initialize", p, DshVersion.InitializeTimeout, cancellationToken);
    }

    /// <summary>
    /// 排入一条用户消息。
    ///
    /// 返回的 <c>messageId</c> 只表示「已经进队列了」—— 上游文档明确写了它
    /// **不**标识后续的助手消息或轮次结束。所以界面不能用它去等结果，
    /// 只能等 <c>session.status</c> 回到 idle。把它当成「发完了」会得到一个
    /// 会提前显示「已完成」的界面。
    /// </summary>
    public Task<JsonNode?> PromptAsync(
        string sessionId,
        string text,
        CancellationToken cancellationToken = default)
    {
        var p = new JsonObject
        {
            ["sessionId"] = sessionId,
            ["contentBlocks"] = new JsonArray
            {
                new JsonObject { ["type"] = "text", ["text"] = text },
            },
        };

        return Transport.RequestAsync("session/prompt", p, DshVersion.PromptAcceptTimeout, cancellationToken);
    }

    public Task<JsonNode?> ShutdownAsync(CancellationToken cancellationToken = default)
        => Transport.RequestAsync("shutdown", new JsonObject(), TimeSpan.FromSeconds(30), cancellationToken);

    private JsonRpcLineTransport Transport =>
        _transport ?? throw new InvalidOperationException("运行时尚未启动");

    public async ValueTask DisposeAsync()
    {
        if (_transport is not null) await _transport.DisposeAsync().ConfigureAwait(false);

        var process = _process;
        if (process is null) return;

        try
        {
            if (!process.HasExited)
            {
                // 先关 stdin（内核把 EOF 接到有界关闭流程），再给一个宽限期。
                try { process.StandardInput.Close(); } catch { }
                if (!process.WaitForExit(5000)) process.Kill(entireProcessTree: true);
            }
        }
        catch { }
        finally
        {
            process.Dispose();
            _process = null;
        }
    }
}
