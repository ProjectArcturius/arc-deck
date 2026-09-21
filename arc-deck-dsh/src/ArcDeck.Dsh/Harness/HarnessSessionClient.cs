using System.Text.Json.Nodes;
using ArcDeck.Dsh.View;

namespace ArcDeck.Dsh.Harness;

/// <summary>harness 会话列表里的一行（<c>session/list</c> 的 value.items）。</summary>
public sealed record HarnessSessionInfo(
    string SessionId,
    DateTimeOffset UpdatedAt,
    bool Running,
    bool Blank,
    string? WorkingDirectory,
    string? ParentSessionId,
    bool IsSubagent)
{
    /// <summary>列表上显示的那行副标题。真实标题在 <c>session/title</c> 事件里，要打开才知道。</summary>
    public string Subtitle
    {
        get
        {
            var parts = new List<string>();
            if (WorkingDirectory is { Length: > 0 }) parts.Add(WorkingDirectory);
            if (IsSubagent) parts.Add("子 agent");
            if (Running) parts.Add("运行中");
            return string.Join(" · ", parts);
        }
    }
}

/// <summary>一次 follow 打开的结果：快照 + 一条可以继续读的流。</summary>
public sealed class HarnessSessionStream
{
    public required DshSessionView View { get; init; }
    public required DshProjection Projection { get; init; }
    public required HarnessStream Stream { get; init; }

    /// <summary>follow 起始游标 —— 之后收到的每条事件 seq 都应大于它。</summary>
    public required long Cursor { get; init; }

    /// <summary>打开时那一页历史之前还有没有更早的记录（有就要用 <c>session/page</c> 往回翻）。</summary>
    public required bool HasMore { get; init; }

    /// <summary>打开时的历史记录条数（不含之后的 live 帧）。</summary>
    public required int SnapshotRecords { get; init; }

    /// <summary>其中**真的折进投影**的事件帧数。</summary>
    public required int FoldedEvents { get; init; }

    /// <summary>
    /// 其中**被跳过**的压缩行条数。
    ///
    /// 这个数与 <see cref="FoldedEvents"/> 一起报出来，「打包行确实跳过了」才是
    /// 一件看得见的事 —— 否则「跳过」与「没收到」在输出上一模一样（参 P9）。
    /// </summary>
    public required int SkippedPackedRows { get; init; }

    /// <summary>
    /// 快照里每种 record 的条数（<c>event assistant/chunk</c> 之类）。
    ///
    /// 与日志侧的计数**不是同一个东西**，见 <c>RecordTypeKey</c> 的说明。
    /// </summary>
    public required Dictionary<string, int> SnapshotRecordTypes { get; init; }
}

/// <summary>
/// harness web 的会话级客户端 —— 界面真正要用的那一层。
///
/// **它与 SDK stdio 那条路共用 <see cref="DshProjection"/>。** 这不是省事，
/// 是正确性要求：两条路喂进来的事件都长成
/// <c>{ type, seq, time, data }</c>（上游的 <c>SessionWireEvent</c>），
/// 折叠规则必须一模一样 —— 否则「界面上看到的」与「从日志读回来的」
/// 会分叉，而那种分叉只在带工具调用的会话上才显形。
///
/// 这两条路各有一个**独立的**验证：
/// <list type="bullet">
///   <item>SDK 路：<c>dsh-cli --verify-reload</c>（实时 vs 重放逐块比对）</item>
///   <item>web 路：<c>dsh-cli --harness-verify</c>（follow 出来的投影 vs 日志重放逐块比对）</item>
/// </list>
/// 后者尤其值钱：它是「Remote 层喂来的事件」与「磁盘上的事实」之间的对账。
/// </summary>
public sealed class HarnessSessionClient : IAsyncDisposable
{
    private readonly HarnessHost _host;
    private readonly HarnessRpc _rpc;
    private readonly HarnessMux _mux;

    private HarnessSessionClient(HarnessHost host, HarnessRpc rpc, HarnessMux mux)
    {
        _host = host;
        _rpc = rpc;
        _mux = mux;
    }

    public HarnessHost Host => _host;

    public static async Task<HarnessSessionClient> ConnectAsync(
        HarnessHost host,
        CancellationToken cancellationToken = default)
        => new(host, new HarnessRpc(host), await HarnessMux.ConnectAsync(host, cancellationToken).ConfigureAwait(false));

    /// <summary>列会话。这是**内核给的**列表，不是扫磁盘 —— 带 running 与 cwd。</summary>
    public async Task<IReadOnlyList<HarnessSessionInfo>> ListSessionsAsync(CancellationToken cancellationToken = default)
    {
        // `_request` 这个名字来自 Host 签名 `list(_request, signal)`。
        // **不是 `request`** —— 实测两个名字不通用，Gateway 按描述符逐字校验。
        var value = await _rpc.CallAsync("session", "list",
            new JsonObject { ["_request"] = new JsonObject() }, cancellationToken).ConfigureAwait(false);

        var items = value?["items"] as JsonArray ?? new JsonArray();
        var result = new List<HarnessSessionInfo>(items.Count);

        foreach (var item in items)
        {
            if (item is not JsonObject o) continue;
            var id = o["sessionId"]?.GetValue<string>();
            if (string.IsNullOrEmpty(id)) continue;

            result.Add(new HarnessSessionInfo(
                SessionId: id!,
                UpdatedAt: FromMs(o["updatedAt"]?.GetValue<long>() ?? 0),
                Running: o["running"]?.GetValue<bool>() ?? false,
                Blank: o["blank"]?.GetValue<bool>() ?? false,
                WorkingDirectory: o["cwd"]?.GetValue<string>(),
                ParentSessionId: o["parentSessionId"]?.GetValue<string>(),
                IsSubagent: o["origin"]?.GetValue<string>() == "subagent"));
        }

        return result;
    }

    /// <summary>
    /// 打开一个会话：读 opening 快照并把它折叠成视图，同时留下流继续读。
    ///
    /// <paramref name="withAssistantStream"/> 打开的话，内核还会推**进程内**的
    /// 助手流帧（没落盘的增量）。界面上要流式观感就得开；
    /// 做「实时 vs 重放」对账时要**关**，否则重放侧没有对应物。
    /// </summary>
    public async Task<HarnessSessionStream> OpenAsync(
        string sessionId,
        bool withAssistantStream = true,
        int maxMessages = 200,
        TimeSpan? openBudget = null,
        CancellationToken cancellationToken = default)
    {
        var request = new JsonObject
        {
            ["address"] = new JsonObject { ["kind"] = "session", ["sessionId"] = sessionId },
            ["maxMessages"] = maxMessages,
        };
        if (withAssistantStream) request["assistantStream"] = true;

        // `follow(request, signal)` → wire 字段是 `request`。
        var stream = await _mux.OpenAsync("session/follow",
            new JsonObject { ["request"] = request }, cancellationToken).ConfigureAwait(false);

        var view = new DshSessionView { SessionId = sessionId, Title = "会话" };
        var projection = new DshProjection(view, replay: !withAssistantStream);

        long cursor = -1;
        var hasMore = false;
        var snapshotRecords = 0;
        var foldedEvents = 0;
        var skippedPackedRows = 0;
        var recordTypes = new Dictionary<string, int>(StringComparer.Ordinal);
        var sawSnapshot = false;

        // 读到快照为止。快照之后是 live 帧，那时才把流交还给调用方 ——
        // 「打开」这个动作必须是**有界**的，否则调用方会等一个永远不结束的流。
        //
        // **「读到快照为止」是字面意思：快照那一帧折完就 `break`。**
        // 上游的契约是 `@returns a complete opening snapshot followed by
        // gap-free event frames` —— 快照是 opening 的**全部**，
        // 之后的每一帧都从 `cursor + 1` 开始连续编号，属于 live 尾巴。
        //
        // 这条曾经写错过，代价是一整轮卡死：旧代码收下快照后**没有跳出循环**，
        // 于是继续等下一帧（一个不再有人说话的会话当然没有下一帧），
        // 一路等到 60s 上限，报出来的还是「没给出快照」——
        // 而线上那一帧 73806 字节的快照早就到了（`probe-harness7.log`）。
        // 「收到了却报没收到」比「没收到」难查得多，所以这里不留含糊。
        //
        // 快照与调用方第一次 `PumpAsync` 之间到达的帧**不会丢**：
        // mux 的 channel 是无界的，帧在里面排队等读者。
        //
        // 这里再加一道**打开上限**：内核可能在别处报错而这条流一声不响
        // （实测撞到过「登录与 list 都成功、follow 永不返回」）。没有上限的话，
        // 界面会停在「正在打开」上，而没有任何东西可以查。
        using var openTimeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        var budget = openBudget ?? TimeSpan.FromSeconds(60);
        openTimeout.CancelAfter(budget);

        while (true)
        {
            HarnessMux.MuxFrame? frame;
            try
            {
                frame = await stream.NextAsync(openTimeout.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
            {
                throw new HarnessRpcException("follow-timeout",
                    $"follow 在 {budget.TotalSeconds:0}s 内没有给出快照（会话 {sessionId}）；" +
                    $"WS 状态={_mux.SocketState}，线上轨迹：\n  {_mux.TraceDump()}", null);
            }
            if (frame is null)
            {
                if (!sawSnapshot) throw new HarnessRpcException("follow-closed", "follow 流在给出快照前就结束了", null);
                break;
            }

            if (frame.Value.Kind == HarnessMux.MuxFrameKind.Error)
            {
                // **这条错误是真实的、必须原样上抛。** 实测撞到过内核自己的报错：
                // 「corrupt Zstandard session log: complete frame contains a torn
                // JSONL record」—— 那是一个正在被写的大会话。
                // 界面上要显示成「这个会话现在打不开」，而不是空白页。
                throw frame.Value.Error ?? new HarnessRpcException("follow-error", "follow 流报错但没带错误", null);
            }

            if (frame.Value.Kind == HarnessMux.MuxFrameKind.End) break;

            var value = frame.Value.Value as JsonObject;
            if (value is null) continue;

            if (value["type"]?.GetValue<string>() != "snapshot")
            {
                // 快照**之前**到达的其余帧：`assistantStream` 打开时内核会推
                // 进程内的助手流帧（没落盘的增量）。投影的输入是**持久事件**，
                // 所以这里只跳过、不折叠。
                continue;
            }

            sawSnapshot = true;
            cursor = value["cursor"]?.GetValue<long>() ?? -1;
            hasMore = value["hasMore"]?.GetValue<bool>() ?? false;
            view.Workspace = value["header"]?["cwd"]?.GetValue<string>() ?? string.Empty;

            foreach (var record in value["records"] as JsonArray ?? new JsonArray())
            {
                switch (ApplyRecord(projection, sessionId, record))
                {
                    case RecordKind.Event: foldedEvents++; break;
                    case RecordKind.Packed: skippedPackedRows++; break;
                }
                var key = RecordTypeKey(record);
                if (key is not null) recordTypes[key] = recordTypes.GetValueOrDefault(key) + 1;
                snapshotRecords++;
            }

            // 打开到此为止 —— 理由见上面那段注释里的上游契约。
            break;
        }

        return new HarnessSessionStream
        {
            View = view,
            Projection = projection,
            Stream = stream,
            Cursor = cursor,
            HasMore = hasMore,
            SnapshotRecords = snapshotRecords,
            FoldedEvents = foldedEvents,
            SkippedPackedRows = skippedPackedRows,
            SnapshotRecordTypes = recordTypes,
        };
    }

    /// <summary>
    /// 把 <c>session/follow</c> 后续的 live 帧折叠进视图，直到 <paramref name="until"/> 取消。
    ///
    /// 返回收到的事件类型直方图 —— 「收到了哪些事件」是排查流式问题的第一手材料，
    /// 而调用方（界面）只关心视图变了没有。
    /// </summary>
    public async Task<Dictionary<string, int>> PumpAsync(
        HarnessSessionStream session,
        Action<DshProjection.Change, DshSessionEvent>? onEvent = null,
        CancellationToken until = default)
    {
        var histogram = new Dictionary<string, int>(StringComparer.Ordinal);

        while (!until.IsCancellationRequested)
        {
            HarnessMux.MuxFrame? frame;
            try
            {
                frame = await session.Stream.NextAsync(until).ConfigureAwait(false);
            }
            catch (OperationCanceledException) { break; }

            if (frame is null || frame.Value.Kind is HarnessMux.MuxFrameKind.End) break;
            if (frame.Value.Kind == HarnessMux.MuxFrameKind.Error) break;

            var value = frame.Value.Value as JsonObject;
            if (value is null) continue;

            var type = value["type"]?.GetValue<string>();
            if (type != "event") continue;

            var evt = ToEvent(session.View.SessionId, value);
            if (evt is null) continue;

            histogram[evt.Type] = histogram.GetValueOrDefault(evt.Type) + 1;
            var change = session.Projection.Apply(evt);
            if (change != DshProjection.Change.None) onEvent?.Invoke(change, evt);
        }

        return histogram;
    }

    /// <summary>
    /// 发一条消息。
    ///
    /// <paramref name="requestId"/> 是客户端铸的关联标识：Host 会把它回显为
    /// 那条持久 user 消息的 <c>rpcId</c>，所以**重试用同一个 id 是安全的** ——
    /// 已经进过队列或日志的请求会直接返回上次的结果，不会重复插入。
    /// 这一点对界面很要紧：网络抖动时重发不该多出一条消息。
    ///
    /// 回执只表示**入队**（<c>accepted: true</c>），不表示回答完成；
    /// 完成要靠 follow 流上的 <c>turn/end</c> / 状态变化来判断。
    /// </summary>
    public async Task<bool> PromptAsync(
        string sessionId,
        string text,
        string mode = "queue",
        string? requestId = null,
        CancellationToken cancellationToken = default)
    {
        var request = new JsonObject
        {
            ["requestId"] = requestId ?? Guid.NewGuid().ToString(),
            ["sessionId"] = sessionId,
            ["mode"] = mode,
            ["content"] = new JsonArray { new JsonObject { ["type"] = "text", ["text"] = text } },
        };

        var value = await _rpc.CallAsync("session", "prompt",
            new JsonObject { ["request"] = request }, cancellationToken).ConfigureAwait(false);

        return value?["accepted"]?.GetValue<bool>() ?? false;
    }

    /// <summary>改名。<c>session/rename</c> 是 SDK stdio 那条路**没有**的能力。</summary>
    public async Task<(string Title, long Seq)> RenameAsync(
        string sessionId,
        string title,
        CancellationToken cancellationToken = default)
    {
        var value = await _rpc.CallAsync("session", "rename", new JsonObject
        {
            ["request"] = new JsonObject { ["sessionId"] = sessionId, ["title"] = title },
        }, cancellationToken).ConfigureAwait(false);

        return (value?["title"]?.GetValue<string>() ?? title, value?["seq"]?.GetValue<long>() ?? -1);
    }

    /// <summary>取消当前轮次，但**保留**待处理的队列。</summary>
    public Task CancelAsync(string sessionId, CancellationToken cancellationToken = default)
        => _rpc.CallAsync("session", "cancel",
            new JsonObject { ["request"] = new JsonObject { ["sessionId"] = sessionId } }, cancellationToken);

    /// <summary>一条 <c>SessionHistoryRecord</c> 折进投影之后算什么。</summary>
    private enum RecordKind
    {
        /// <summary>既不是 event 也不是已知的打包行 —— 记账用，不折叠。</summary>
        Other,

        /// <summary>真的折进投影了一帧事件。</summary>
        Event,

        /// <summary>压缩行，按 P7 的规矩跳过。</summary>
        Packed,
    }

    /// <summary>
    /// 一条 record 的类型键，形如 <c>event assistant/chunk</c> 或
    /// <c>chunks chunkrow/text-chunks</c>。
    ///
    /// 存在的理由是「4 行 vs 10 行」那种数不上：日志侧的压缩行计数数的是**文件里
    /// 的行**，线上侧的计数数的是**内核重新打包后的记录** —— 两个数不可直接相减。
    /// 把 record 的真实构成打出来，才知道差在编码还是在内容（参 P9：
    /// 「跳过打包行」这件事要能看见，不能只论证）。
    /// </summary>
    private static string? RecordTypeKey(JsonNode? record)
    {
        if (record is not JsonObject entry) return null;
        var kind = entry["type"]?.GetValue<string>();
        if (string.IsNullOrEmpty(kind)) return null;

        var inner = entry["event"]?["type"]?.GetValue<string>();
        return string.IsNullOrEmpty(inner) ? kind : $"{kind} {inner}";
    }

    /// <summary>
    /// 把一条 <c>SessionHistoryRecord</c> 折叠进投影。
    ///
    /// **两种 record 都用 <c>event</c> 这个键**，靠 <c>type</c> 判别
    /// （从 <c>pageRecords</c> 读出）：
    ///
    /// <code>
    /// { type: 'event',  event: &lt;SessionWireEvent&gt; }
    /// { type: 'chunks', event: { type: 'chunkrow/text-chunks', seq, time, data } }
    /// </code>
    ///
    /// 后一种是「连续同类增量块的压缩行」，与磁盘日志里的 <c>text-chunks</c> 是同一件事，
    /// 但**线上换了名字**（多了 <c>chunkrow/</c> 前缀）—— 所以
    /// <see cref="DshEventType.PackedTextChunks"/> 那个常量在这儿是**匹配不到**的。
    ///
    /// 正因如此，这里按 <c>type</c> 直接跳过打包行，而不是把它变回事件 ——
    /// **不要**把它「简化」成「所有 record 的 event 都折叠」：
    /// 那样打包行会以未知类型混进投影，而 P7 的教训是展开打包行会让正文写两遍。
    /// 报出 <see cref="RecordKind.Packed"/> 的计数是为了让「确实跳过了」可被看见。
    /// </summary>
    private static RecordKind ApplyRecord(DshProjection projection, string sessionId, JsonNode? record)
    {
        if (record is not JsonObject entry) return RecordKind.Other;

        var kind = entry["type"]?.GetValue<string>();
        if (kind == "chunks") return RecordKind.Packed;
        if (kind != "event") return RecordKind.Other;

        var evt = ToEvent(sessionId, entry["event"]);
        if (evt is null) return RecordKind.Other;

        projection.Apply(evt);
        return RecordKind.Event;
    }

    /// <summary>
    /// 把上游的 <c>SessionWireEvent</c> 变成本层的 <see cref="DshSessionEvent"/>。
    ///
    /// 两者字段**一一对应**（<c>type</c> / <c>seq</c> / <c>time</c> / <c>data</c>），
    /// 这正是两条路能共用投影的原因。多出来的 <c>surfaceOp</c>、
    /// <c>sourceEventSeqs</c>、<c>ignorable</c> 目前不影响折叠规则。
    /// </summary>
    private static DshSessionEvent? ToEvent(string sessionId, JsonNode? node)
    {
        if (node is not JsonObject e) return null;

        var type = e["type"]?.GetValue<string>();
        if (string.IsNullOrEmpty(type)) return null;

        var timeMs = e["time"]?.GetValue<long>() ?? 0;
        return new DshSessionEvent(
            SessionId: sessionId,
            Type: type!,
            Sequence: e["seq"]?.GetValue<long>() ?? -1,
            Time: timeMs > 0 ? FromMs(timeMs) : DateTimeOffset.Now,
            Data: e["data"],
            Raw: e);
    }

    private static DateTimeOffset FromMs(long ms) =>
        ms > 0 ? DateTimeOffset.FromUnixTimeMilliseconds(ms).ToLocalTime() : DateTimeOffset.Now;

    public async ValueTask DisposeAsync()
    {
        await _mux.DisposeAsync().ConfigureAwait(false);
        _rpc.Dispose();
        await _host.DisposeAsync().ConfigureAwait(false);
    }
}
