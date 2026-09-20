using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace ArcDeck.Dsh.Transport;

/// <summary>
/// 按 <c>\n</c> 分帧的 JSON-RPC 2.0 —— 上游 <c>@deepseek-ai/dsh-sdk-protocol</c>
/// 的 <c>JsonRpcLineTransport</c> 的对侧实现。
///
/// 上游的判定规则（照抄，不自作主张）：
///   · 同时有 <c>id</c> 与 <c>method</c> → 请求
///   · 只有 <c>id</c> → 响应
///   · 只有 <c>method</c> → 通知
///   · **解析不了的行直接忽略**，不当错误
///
/// 最后一条值得抄：内核在启动早期可能往 stdout 写非协议内容（警告之类），
/// 把这些当成致命错误会让「其实能跑」的部署被判死。
/// </summary>
public sealed class JsonRpcLineTransport : IAsyncDisposable
{
    private readonly Stream _input;
    private readonly Stream _output;
    private readonly object _writeGate = new();
    private readonly Dictionary<string, TaskCompletionSource<JsonNode?>> _pending = new();
    private readonly CancellationTokenSource _cts = new();

    private Task? _pump;
    private long _nextId;

    /// <summary>每一条**通知**（服务端 → 客户端）。抛异常不会弄死读取循环。</summary>
    public event Action<string, JsonNode?>? NotificationReceived;

    /// <summary>非协议行与诊断输出。内核的 stderr 也走这里 —— 排查时最有用的一条线。</summary>
    public event Action<string>? Diagnostic;

    public bool IsClosed { get; private set; }

    public JsonRpcLineTransport(Stream input, Stream output)
    {
        _input = input;
        _output = output;
    }

    public void Start()
    {
        _pump ??= Task.Run(() => PumpAsync(_cts.Token));
    }

    /// <summary>
    /// 发一条请求并等它的响应。
    ///
    /// 超时是**必须**的，而且默认值不能太短：内核第一次 <c>initialize</c> 要等
    /// 整个插件树加载完（上游文档里明确写了它会 <c>await loader</c>），
    /// 在一台已经跑着别的东西的机器上十几秒是正常的。给 3 秒会让它在慢机器上
    /// 随机失败，而那种失败看起来像「协议不对」。
    /// </summary>
    public async Task<JsonNode?> RequestAsync(
        string method,
        JsonObject? parameters,
        TimeSpan? timeout = null,
        CancellationToken cancellationToken = default)
    {
        if (IsClosed) throw new InvalidOperationException("传输已关闭");

        var id = $"req_{Interlocked.Increment(ref _nextId)}_{Guid.NewGuid():N}";
        var frame = new JsonObject
        {
            ["jsonrpc"] = "2.0",
            ["id"] = id,
            ["method"] = method,
        };
        if (parameters is not null) frame["params"] = parameters;

        var tcs = new TaskCompletionSource<JsonNode?>(TaskCreationOptions.RunContinuationsAsynchronously);
        lock (_pending) _pending[id] = tcs;

        WriteFrame(frame);

        var budget = timeout ?? TimeSpan.FromMinutes(3);
        using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutCts.CancelAfter(budget);

        try
        {
            return await tcs.Task.WaitAsync(timeoutCts.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            lock (_pending) _pending.Remove(id);
            throw new TimeoutException($"{method} 在 {budget.TotalSeconds:F0}s 内没有响应");
        }
    }

    public void Notify(string method, JsonObject? parameters = null)
    {
        var frame = new JsonObject { ["jsonrpc"] = "2.0", ["method"] = method };
        if (parameters is not null) frame["params"] = parameters;
        WriteFrame(frame);
    }

    private void WriteFrame(JsonObject frame)
    {
        var line = frame.ToJsonString(JsonOptions) + "\n";
        var bytes = Encoding.UTF8.GetBytes(line);

        // 单写者闸门：内核的 stdin 只有一个，两块并发写会互相撕开帧。
        lock (_writeGate)
        {
            _output.Write(bytes, 0, bytes.Length);
            _output.Flush();
        }
    }

    private async Task PumpAsync(CancellationToken cancellationToken)
    {
        // 用 StreamReader 而不是自己拼字节缓冲：
        // UTF-8 的多字节字符会跨 read 边界断开，自己拼要写解码器状态机。
        using var reader = new StreamReader(_input, new UTF8Encoding(false), false, 8192, leaveOpen: true);

        try
        {
            while (!cancellationToken.IsCancellationRequested)
            {
                var line = await reader.ReadLineAsync(cancellationToken).ConfigureAwait(false);
                if (line is null) break;              // EOF：内核退出了
                if (line.Length == 0) continue;

                JsonNode? node;
                try { node = JsonNode.Parse(line); }
                catch (JsonException)
                {
                    // 上游明确要求：解析不了的行**忽略**。这里只留一条诊断。
                    Diagnostic?.Invoke($"[非协议行] {Truncate(line, 300)}");
                    continue;
                }

                Dispatch(node);
            }
        }
        catch (OperationCanceledException) { }
        catch (Exception ex)
        {
            Diagnostic?.Invoke($"[读取循环异常] {ex.GetType().Name}: {ex.Message}");
        }
        finally
        {
            IsClosed = true;
            FailAllPending("JSON-RPC 输入已关闭");
        }
    }

    private void Dispatch(JsonNode? node)
    {
        if (node is not JsonObject frame) return;

        var hasId = frame.TryGetPropertyValue("id", out var idNode) && idNode is not null;
        var hasMethod = frame.TryGetPropertyValue("method", out var methodNode)
                        && methodNode?.GetValue<string>() is { Length: > 0 } method;

        if (hasId && hasMethod)
        {
            // 服务端 → 客户端请求：本协议**没有**这种消息（上游文档写明未使用）。
            // 回一条「方法不存在」，而不是静默丢弃 —— 静默丢弃会让将来真加了
            // 这种消息时表现为「内核卡住」。
            method = methodNode!.GetValue<string>();
            WriteFrame(new JsonObject
            {
                ["jsonrpc"] = "2.0",
                ["id"] = idNode!.DeepClone(),
                ["error"] = new JsonObject { ["code"] = -32601, ["message"] = $"method not found: {method}" },
            });
            return;
        }

        if (hasId)
        {
            var id = idNode!.GetValue<string>();
            TaskCompletionSource<JsonNode?>? tcs;
            lock (_pending)
            {
                if (!_pending.Remove(id, out tcs)) return;
            }

            if (frame.TryGetPropertyValue("error", out var error) && error is JsonObject err)
            {
                var code = err.TryGetPropertyValue("code", out var c) ? c?.ToString() ?? "?" : "?";
                var message = err.TryGetPropertyValue("message", out var m) ? m?.GetValue<string>() : "JSON-RPC error";
                tcs.SetException(new JsonRpcException(code, message ?? "JSON-RPC error"));
            }
            else
            {
                tcs.SetResult(frame.TryGetPropertyValue("result", out var result) ? result : null);
            }
            return;
        }

        if (hasMethod)
        {
            try { NotificationReceived?.Invoke(methodNode!.GetValue<string>(), frame.TryGetPropertyValue("params", out var p) ? p : null); }
            catch (Exception ex)
            {
                // 订阅方的异常绝不能弄死读取循环 —— 否则一次界面 bug 会让整个会话「断线」。
                Diagnostic?.Invoke($"[通知处理器异常] {ex.GetType().Name}: {ex.Message}");
            }
        }
    }

    private void FailAllPending(string reason)
    {
        List<TaskCompletionSource<JsonNode?>> waiters;
        lock (_pending)
        {
            waiters = _pending.Values.ToList();
            _pending.Clear();
        }
        foreach (var w in waiters) w.TrySetException(new IOException(reason));
    }

    private static string Truncate(string s, int max) => s.Length <= max ? s : s[..max] + "…";

    internal static readonly JsonSerializerOptions JsonOptions = new()
    {
        WriteIndented = false,
        // 中文不转义 —— 内核日志与消息正文大量是中文，转义后的帧无法人工读。
        Encoder = System.Text.Encodings.Web.JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
    };

    public async ValueTask DisposeAsync()
    {
        _cts.Cancel();
        if (_pump is not null)
        {
            try { await _pump.ConfigureAwait(false); } catch { }
        }
        FailAllPending("传输已释放");
        _cts.Dispose();
    }
}

public sealed class JsonRpcException : Exception
{
    public string Code { get; }
    public JsonRpcException(string code, string message) : base($"[{code}] {message}") => Code = code;
}
