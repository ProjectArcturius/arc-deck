using System.Net.WebSockets;
using System.Text;
using System.Text.Json.Nodes;
using System.Threading.Channels;

namespace ArcDeck.Dsh.Harness;

/// <summary>
/// Remote 层的逻辑流：<c>ws://…/api/remote.mux</c>。
///
/// ## 帧形状（从 harness 源码读出并实测）
///
/// 客户端 → 服务端：
/// <code>
/// { "type":"open",   "streamId":"…", "endpoint":"session/follow", "payload":{ "args": {…} } }
/// { "type":"cancel", "streamId":"…" }
/// </code>
///
/// 服务端 → 客户端：
/// <code>
/// { "type":"item",  "streamId":"…", "value":… }   // value 可缺省
/// { "type":"error", "streamId":"…", "error":{ code, message, details } }
/// { "type":"end",   "streamId":"…" }
/// </code>
///
/// 一条物理 WebSocket 上可以开多条逻辑流（靠 <c>streamId</c> 分流），
/// 但本客户端每个订阅各开一条 —— 少了多路复用的簿记，
/// 而 WebSocket 连接的代价在这个规模上可以忽略。
///
/// ## 认证走 Cookie 头
///
/// 浏览器的 <c>WebSocket</c> 构造器**不允许**自定义头，所以浏览器客户端只能
/// 依赖 cookie 自动携带。.NET 的 <see cref="ClientWebSocket"/> 允许显式设置头，
/// 于是这里把换票拿到的 cookie 直接写进 Upgrade 请求 ——
/// 比依赖 cookie 容器更可预测，而那条路由（`api/remote.mux`）本来就在
/// 「浏览器会话」的信任边界内。
/// </summary>
public sealed class HarnessMux : IAsyncDisposable
{
    private readonly HarnessHost _host;
    private readonly ClientWebSocket _socket = new();
    private readonly Channel<MuxFrame> _frames = Channel.CreateUnbounded<MuxFrame>();
    private readonly CancellationTokenSource _cts = new();

    private Task? _pump;
    private bool _disposed;

    private HarnessMux(HarnessHost host) => _host = host;

    /// <summary>
    /// 最近收到/发出的原始帧（有上限）。排查「流开了但一个 item 都没有」时，
    /// 唯一的办法就是看**线上到底有没有东西** —— 猜不出来。
    /// </summary>
    public Queue<string> Trace { get; } = new();

    private void Note(string line)
    {
        lock (Trace)
        {
            Trace.Enqueue($"{DateTimeOffset.Now:HH:mm:ss.fff} {line}");
            while (Trace.Count > 50) Trace.Dequeue();
        }
    }

    public string TraceDump()
    {
        lock (Trace) return string.Join("\n  ", Trace);
    }

    /// <summary>WebSocket 的当前状态 —— 排查「流为什么不动」时要看的第一个东西。</summary>
    public WebSocketState SocketState => _socket.State;

    /// <summary>一条逻辑流的一帧。</summary>
    public readonly record struct MuxFrame(MuxFrameKind Kind, JsonNode? Value, HarnessRpcException? Error);

    public enum MuxFrameKind { Item, End, Error }

    public static async Task<HarnessMux> ConnectAsync(HarnessHost host, CancellationToken cancellationToken = default)
    {
        var mux = new HarnessMux(host);
        var wsUrl = new UriBuilder(host.Origin) { Scheme = "ws", Path = "/api/remote.mux" }.Uri;

        mux._socket.Options.SetRequestHeader("Cookie", host.AuthCookie);

        // 连接也要有上限。没有它的话，握手被拒（例如 cookie 不被接受）会让
        // 调用方**永久挂住**，而那种卡死看起来像「内核在思考」——
        // 实测第一次就是这么卡住的：登录成功、list 也成功，然后 follow 再也不返回。
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(TimeSpan.FromSeconds(20));
        await mux._socket.ConnectAsync(wsUrl, timeout.Token).ConfigureAwait(false);

        mux._pump = Task.Run(() => mux.PumpAsync(mux._cts.Token), CancellationToken.None);
        return mux;
    }

    /// <summary>
    /// 开一条逻辑流。
    ///
    /// 返回值是**阻塞式的枚举**，调用方自己决定读到哪一帧停 ——
    /// 这里刻意不做「收满 N 帧就关」那种便利接口：`session/follow` 是长连流，
    /// 「什么时候算够」只有调用方知道（界面是「用户切走」才停）。
    /// </summary>
    public async Task<HarnessStream> OpenAsync(
        string endpoint,
        JsonObject args,
        CancellationToken cancellationToken = default)
    {
        var streamId = Guid.NewGuid().ToString();
        var open = new JsonObject
        {
            ["type"] = "open",
            ["streamId"] = streamId,
            ["endpoint"] = endpoint,
            ["payload"] = new JsonObject { ["args"] = args },
        };

        await SendAsync(open, cancellationToken).ConfigureAwait(false);
        Note($"[open] {endpoint} streamId={streamId[..8]}");
        return new HarnessStream(this, streamId);
    }

    internal async Task SendAsync(JsonObject frame, CancellationToken cancellationToken)
    {
        var bytes = Encoding.UTF8.GetBytes(frame.ToJsonString(Transport.JsonRpcLineTransport.JsonOptions));
        await _socket.SendAsync(bytes, WebSocketMessageType.Text, true, cancellationToken).ConfigureAwait(false);
    }

    /// <summary>异步读一条逻辑流的下一帧。流结束时返回 null。</summary>
    internal async Task<MuxFrame?> NextAsync(CancellationToken cancellationToken)
    {
        try
        {
            return await _frames.Reader.ReadAsync(cancellationToken).ConfigureAwait(false);
        }
        catch (ChannelClosedException)
        {
            return null;
        }
    }

    private async Task PumpAsync(CancellationToken cancellationToken)
    {
        var buffer = new byte[1 << 16];

        // **必须攒完整条消息再解码，不能逐块解码。**
        //
        // 这是实测抓到的真 bug，不是洁癖：旧的写法在 `do` 循环里对每个
        // `ReceiveAsync` 分块各自调 `Encoding.UTF8.GetString`，
        // 于是一个**跨分块边界的多字节字符**会被两边各解出半个 ——
        // 变成两个 U+FFFD，而且字符串还多出一个字符。
        //
        // 症状实测（`--harness-check-all`，会话 session-0d696d55）：
        // 147 个块里只有一个思考块长度是 7799 而重放是 7798，差的那 1 个字符就是它：
        //
        //   实时 …帧（Delay → 隐\uFFFD\uFFFD）\nassert!(    ← 1 个汉字裂成 2 个替换字符
        //   重放 …帧（Delay → 隐藏）\nassert!(r
        //
        // 也就是说：**中文内容在长消息里会被静默损坏**。快照动辄几十上百 KB、
        // 一定跨分块，而中文字符占 3 字节，所以这几乎必然发生 ——
        // 之前在短会话上没暴露，只是那个分块边界恰好没落在字符中间。
        //
        // 攒字节再解一次同时也更快（少了每块的字符串拼接）。
        using var message = new MemoryStream(buffer.Length * 2);

        try
        {
            while (!cancellationToken.IsCancellationRequested && _socket.State == WebSocketState.Open)
            {
                message.SetLength(0);
                WebSocketReceiveResult result;
                do
                {
                    result = await _socket.ReceiveAsync(buffer, cancellationToken).ConfigureAwait(false);
                    if (result.MessageType == WebSocketMessageType.Close)
                    {
                        _frames.Writer.TryComplete();
                        return;
                    }
                    message.Write(buffer, 0, result.Count);
                }
                while (!result.EndOfMessage);

                // 到这里才是一条**完整**消息：UTF-8 序列不会再有半截的。
                var text = Encoding.UTF8.GetString(message.GetBuffer(), 0, (int)message.Length);

                Note($"[recv] {message.Length} 字节 {text[..Math.Min(120, text.Length)]}");
                Dispatch(text);
            }
        }
        catch (OperationCanceledException) { }
        catch (Exception ex)
        {
            Note($"[pump 异常] {ex.GetType().Name}: {ex.Message}");
            _frames.Writer.TryWrite(new MuxFrame(MuxFrameKind.Error, null,
                new HarnessRpcException("mux-failure", ex.Message, null)));
        }
        finally
        {
            _frames.Writer.TryComplete();
        }
    }

    private void Dispatch(string text)
    {
        JsonNode? node;
        try { node = JsonNode.Parse(text); }
        catch (System.Text.Json.JsonException) { return; }

        if (node is not JsonObject frame) return;
        var streamId = frame["streamId"]?.GetValue<string>();
        if (string.IsNullOrEmpty(streamId)) return;

        switch (frame["type"]?.GetValue<string>())
        {
            case "item":
                _frames.Writer.TryWrite(new MuxFrame(MuxFrameKind.Item,
                    frame.TryGetPropertyValue("value", out var v) ? v : null, null));
                break;

            case "end":
                _frames.Writer.TryWrite(new MuxFrame(MuxFrameKind.End, null, null));
                break;

            case "error":
                var error = frame["error"] as JsonObject;
                _frames.Writer.TryWrite(new MuxFrame(MuxFrameKind.Error, null, new HarnessRpcException(
                    error?["code"]?.GetValue<string>() ?? "unknown",
                    error?["message"]?.GetValue<string>() ?? "（无消息）",
                    error?["details"])));
                break;
        }
    }

    public async ValueTask DisposeAsync()
    {
        if (_disposed) return;
        _disposed = true;

        _cts.Cancel();
        _frames.Writer.TryComplete();

        try
        {
            if (_socket.State == WebSocketState.Open)
            {
                await _socket.CloseAsync(WebSocketCloseStatus.NormalClosure, "bye", CancellationToken.None)
                    .ConfigureAwait(false);
            }
        }
        catch { }
        finally
        {
            _socket.Dispose();
            _cts.Dispose();
        }
    }
}

/// <summary>一条逻辑流。</summary>
public sealed class HarnessStream
{
    private readonly HarnessMux _mux;

    internal HarnessStream(HarnessMux mux, string streamId)
    {
        _mux = mux;
        StreamId = streamId;
    }

    public string StreamId { get; }

    public Task<HarnessMux.MuxFrame?> NextAsync(CancellationToken cancellationToken = default)
        => _mux.NextAsync(cancellationToken);

    /// <summary>取消这条逻辑流（不影响同一条连接上的其他流）。</summary>
    public Task CancelAsync(CancellationToken cancellationToken = default)
        => _mux.SendAsync(new JsonObject { ["type"] = "cancel", ["streamId"] = StreamId }, cancellationToken);
}
