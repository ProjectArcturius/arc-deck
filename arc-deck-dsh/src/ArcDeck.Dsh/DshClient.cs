using System.Text.Json.Nodes;
using ArcDeck.Dsh.Runtime;
using ArcDeck.Dsh.View;

namespace ArcDeck.Dsh;

/// <summary>一次会话级通知：某个会话的视图发生了什么变化。</summary>
public sealed class DshSessionChangedEventArgs : EventArgs
{
    public required DshSessionView View { get; init; }
    public required DshProjection.Change Change { get; init; }
    public required DshSessionEvent Event { get; init; }
}

/// <summary>agent 运行状态变化（<c>session.status</c>）。</summary>
public sealed class DshStatusChangedEventArgs : EventArgs
{
    public required string SessionId { get; init; }
    public required bool IsRunning { get; init; }
}

/// <summary>
/// 面向界面的门面：把 <see cref="DshRuntime"/> 的原始通知变成
/// 「哪个会话的视图变了」。
///
/// 界面对协议的唯一接口就是这里。刻意**不**暴露 <see cref="JsonNode"/> 给界面 ——
/// 一旦界面开始自己读 JSON，翻译规则就有两份，而它们会在上游变动时分叉。
/// </summary>
public sealed class DshClient : IAsyncDisposable
{
    private readonly Dictionary<string, DshSessionView> _views = new(StringComparer.Ordinal);
    private readonly Dictionary<string, DshProjection> _projections = new(StringComparer.Ordinal);
    private readonly DshRuntime _runtime;

    public event EventHandler<DshSessionChangedEventArgs>? SessionChanged;
    public event EventHandler<DshStatusChangedEventArgs>? StatusChanged;

    /// <summary>诊断行（内核 stderr、非协议输出）。</summary>
    public event EventHandler<string>? Diagnostic;

    public DshRuntimeLayout Layout => _runtime.Layout;

    /// <summary>已经产生过投影的会话 —— 界面用它填列表。</summary>
    public IReadOnlyCollection<DshSessionView> Sessions => _views.Values;

    private DshClient(DshRuntime runtime)
    {
        _runtime = runtime;
        _runtime.NotificationReceived += OnNotification;
        _runtime.Diagnostic += m => Diagnostic?.Invoke(this, m);
    }

    /// <summary>启动内核并握手。返回 null 表示这台机器上没有可用的 dsh 安装。</summary>
    public static async Task<DshClient?> ConnectAsync(
        string workspaceDirectory,
        Action<string>? diagnostic = null,
        CancellationToken cancellationToken = default)
    {
        var runtime = await DshRuntime.StartAsync(diagnostic: diagnostic, cancellationToken: cancellationToken)
            .ConfigureAwait(false);
        if (runtime is null) return null;

        var client = new DshClient(runtime);
        try
        {
            await runtime.InitializeAsync(workspaceDirectory, cancellationToken: cancellationToken)
                .ConfigureAwait(false);
        }
        catch
        {
            await client.DisposeAsync().ConfigureAwait(false);
            throw;
        }

        return client;
    }

    /// <summary>为某个工作区开一个会话视图。第一次 <c>session/prompt</c> 时内核才真正建 agent。</summary>
    public DshSessionView OpenSession(string sessionId, string workspace, string title = "新会话")
    {
        if (_views.TryGetValue(sessionId, out var existing)) return existing;

        var view = new DshSessionView { SessionId = sessionId, Workspace = workspace, Title = title };
        _views[sessionId] = view;
        _projections[sessionId] = new DshProjection(view);
        return view;
    }

    /// <summary>
    /// 取某个会话的投影器。
    ///
    /// **投影只能有一份**。调用方自己 new 一个虽然也能往同一个 view 上写，
    /// 但两份状态机见到的事件序列不同（一份从第一条开始、另一份从中途开始），
    /// 结果会随「谁先看到哪一帧」而分叉 —— 这类 bug 只在特定时序下出现。
    /// </summary>
    public DshProjection ProjectionOf(string sessionId)
        => _projections.TryGetValue(sessionId, out var p)
            ? p
            : throw new InvalidOperationException($"会话 {sessionId} 还没有打开");

    /// <summary>
    /// 发一条消息。回执只表示**入队**，不表示回答完成 —— 上游文档明确写了
    /// 没有「逐提示词结果」，完成只能靠 <c>session.status</c> 回到 idle。
    /// </summary>
    public async Task<string?> SendAsync(string sessionId, string text, CancellationToken cancellationToken = default)
    {
        var result = await _runtime.PromptAsync(sessionId, text, cancellationToken).ConfigureAwait(false);
        return result?["messageId"]?.GetValue<string>();
    }

    private void OnNotification(string method, JsonNode? payload)
    {
        switch (method)
        {
            case "session.event":
                OnSessionEvent(payload);
                break;

            case "session.status":
                OnSessionStatus(payload);
                break;

            // subagent 的两种通知暂时不进界面。**不是漏了** —— 子 agent 的过程
            // 在屏幕上会盖住主线；等界面有折叠容器再放进来。
            case "subagent.started":
            case "subagent.finished":
                break;

            default:
                Diagnostic?.Invoke(this, $"[未知通知] {method}");
                break;
        }
    }

    private void OnSessionEvent(JsonNode? payload)
    {
        if (payload is not JsonObject envelope) return;

        var sessionId = envelope["sessionId"]?.GetValue<string>();
        if (string.IsNullOrEmpty(sessionId)) return;

        var evt = DshSessionEvent.FromNotification(sessionId!, envelope["event"]);
        if (evt is null) return;

        if (!_projections.TryGetValue(sessionId!, out var projection))
        {
            // 内核会把**所有**会话的事件都推过来（上游文档：不过滤）。
            // 没打开过的会话先建一个视图，否则用户切过去时会看到空会话。
            OpenSession(sessionId!, workspace: string.Empty);
            projection = _projections[sessionId!];
        }

        var change = projection.Apply(evt);
        if (change == DshProjection.Change.None) return;

        SessionChanged?.Invoke(this, new DshSessionChangedEventArgs
        {
            View = projection.View,
            Change = change,
            Event = evt,
        });
    }

    private void OnSessionStatus(JsonNode? payload)
    {
        if (payload is not JsonObject p) return;

        var sessionId = p["sessionId"]?.GetValue<string>();
        if (string.IsNullOrEmpty(sessionId)) return;

        var running = string.Equals(p["status"]?.GetValue<string>(), "running", StringComparison.Ordinal);

        if (_views.TryGetValue(sessionId!, out var view)) view.IsRunning = running;

        StatusChanged?.Invoke(this, new DshStatusChangedEventArgs
        {
            SessionId = sessionId!,
            IsRunning = running,
        });
    }

    public async ValueTask DisposeAsync()
    {
        // 先走协议的 shutdown（上游会 dispose 根运行时并落盘），再断进程。
        // 直接 Kill 会让最后一段会话日志丢掉 —— 表现是「重开会话少了几条消息」。
        try
        {
            if (_runtime.IsRunning)
            {
                await _runtime.ShutdownAsync().ConfigureAwait(false);
            }
        }
        catch (Exception ex)
        {
            Diagnostic?.Invoke(this, $"[shutdown 失败，直接断进程] {ex.Message}");
        }
        finally
        {
            await _runtime.DisposeAsync().ConfigureAwait(false);
        }
    }
}
