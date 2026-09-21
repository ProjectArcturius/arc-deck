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
    bool IsSubagent,
    /// <summary>
    /// 子 agent 的调度方式（<c>one-shot</c> / <c>continuable</c>）。
    ///
    /// **这是子 agent 地址的必需字段**，来自列表项里那个顶层 <c>subagent</c> 对象：
    /// <c>SessionAddress</c> 原先只会发普通会话的形状，于是 44 个子 agent 一律被拒为
    /// 「subagent Sessions require their durable parent address」（参 P13）。
    /// </summary>
    string? SubagentMode = null,
    /// <summary>子 agent 自己的名字（<c>subagent.label</c>）—— 列表上比 cwd 有用。</summary>
    string? SubagentLabel = null,
    /// <summary>
    /// 会话标题，来自 <c>projections.values.title</c>。
    ///
    /// 注意这与 SDK 那条路的结论**不一样**：`AGENTS.md` 里写着「列表上拿不到标题，
    /// 真标题要重载才知道」—— 那说的是**扫磁盘**（`DshSessionReader.Enumerate` 只读会话头）。
    /// 而内核给的列表**带标题**，所以 harness 那条路的列表可以有真标题。
    /// </summary>
    string? Title = null,
    /// <summary>
    /// 列表项**原样**保留一份。
    ///
    /// 不是懒：协议没有版本协商（P2），而我们已经因为「schema 里有、实际却不发」
    /// 猜错过一次（`subagent.mode` 就在这一类）。留着原文，下次再对不上时
    /// 不用再写一个探针去把同一份 JSON 捞回来。
    /// </summary>
    JsonObject? Raw = null)
{
    /// <summary>这条路打开这个会话时要用的地址（普通会话与子 agent 是两种形状）。</summary>
    public HarnessAddress Address => new(SessionId, ParentSessionId, SubagentMode);

    /// <summary>列表上显示的那行副标题。</summary>
    public string Subtitle
    {
        get
        {
            var parts = new List<string>();
            if (Title is { Length: > 0 }) parts.Add(Title);
            if (SubagentLabel is { Length: > 0 }) parts.Add($"「{SubagentLabel}」");
            if (WorkingDirectory is { Length: > 0 } && Title is not { Length: > 0 }) parts.Add(WorkingDirectory);
            if (IsSubagent) parts.Add("子 agent");
            if (Running) parts.Add("运行中");
            return string.Join(" · ", parts);
        }
    }
}

/// <summary>
/// 一个会话的**地址** —— 普通会话与子 agent 是两种形状。
///
/// 从上游 `typert.host.js` 的 schema 读出：
/// <code>
/// { kind:'session',   sessionId }
/// { kind:'subagent',  parentSessionId, childSessionId, mode:'one-shot'|'continuable' }
/// </code>
/// 两种形状**不通用**：用普通地址开子 agent 会被拒为
/// 「subagent Sessions require their durable parent address」。参 P13。
/// </summary>
public sealed record HarnessAddress(string SessionId, string? ParentSessionId = null, string? SubagentMode = null)
{
    /// <summary>子 agent 地址要四样齐全，缺一样就只能按普通会话发（然后被内核拒）。</summary>
    public bool IsSubagent =>
        !string.IsNullOrEmpty(ParentSessionId) && !string.IsNullOrEmpty(SubagentMode);

    public static HarnessAddress OfSession(string sessionId) => new(sessionId);

    public JsonObject ToJson() => IsSubagent
        ? new JsonObject
        {
            ["kind"] = "subagent",
            ["parentSessionId"] = ParentSessionId,
            ["childSessionId"] = SessionId,
            ["mode"] = SubagentMode,
        }
        : new JsonObject { ["kind"] = "session", ["sessionId"] = SessionId };
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
    /// 这一页**上面还叠了几页更早的**（<c>session/page</c> 往回翻了几次）。
    ///
    /// 打开一个会话时是 1（只有 follow 给的那一页快照）。要让界面上的
    /// 历史完整，或者要让「线上 vs 日志」的对账覆盖整份日志，就得往回翻。
    /// </summary>
    public required int PagesLoaded { get; init; }

    /// <summary>
    /// 更早的历史**是否已经全部取回**。
    ///
    /// 与 <see cref="HasMore"/> 的分工：那个说的是「内核那边还有」，
    /// 这个说的是「我们已经拿到了」。只有这个为 true，与整份日志的对账才成立 ——
    /// 否则比的是**一页对全部**，差异里全是「没翻到的部分」（参 P13）。
    /// </summary>
    public required bool HistoryComplete { get; init; }

    /// <summary>
    /// 快照里每种 record 的条数（<c>event assistant/chunk</c> 之类）。
    ///
    /// 与日志侧的计数**不是同一个东西**，见 <c>RecordTypeKey</c> 的说明。
    /// </summary>
    public required Dictionary<string, int> SnapshotRecordTypes { get; init; }
}

/// <summary>一次 <c>session/page</c> 的结果：一页更早的历史 + 还有没有更早的。</summary>
public sealed class HarnessHistoryPage
{
    public HarnessHistoryPage(JsonArray records, bool hasMore)
    {
        Records = records;
        HasMore = hasMore;
    }

    public JsonArray Records { get; }
    public bool HasMore { get; }
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

            var values = o["projections"]?["values"];

            // 子 agent 描述符在 **projections.values.subagent** 里，形状 `{ mode, label?, seq }`。
            //
            // 这一处曾经读错**位置**：上游 schema 打印出来的缩进让人以为它是列表项的
            // 顶层字段（`items[].subagent`），于是 `o["subagent"]` 永远是 null、
            // 44 个子 agent 继续用普通地址去开、继续被拒。
            // **原文才是事实**：`--harness-list-raw` 打出来一看，
            // 连一个空会话的 values 里都有 `"subagent": null`。
            // 所以这里先读 values，再退回顶层（两种情况都容错）。
            var subagent = (values?["subagent"] as JsonObject) ?? (o["subagent"] as JsonObject);

            result.Add(new HarnessSessionInfo(
                SessionId: id!,
                UpdatedAt: FromMs(o["updatedAt"]?.GetValue<long>() ?? 0),
                Running: o["running"]?.GetValue<bool>() ?? false,
                Blank: o["blank"]?.GetValue<bool>() ?? false,
                WorkingDirectory: o["cwd"]?.GetValue<string>(),
                ParentSessionId: o["parentSessionId"]?.GetValue<string>(),
                IsSubagent: o["origin"]?.GetValue<string>() == "subagent",
                // 子 agent 描述符是**列表项里一个顶层对象**（不是 projections 里的），
                // 形状：{ mode, label?, seq }。mode 是打开它时地址的必需字段。
                SubagentMode: subagent?["mode"]?.GetValue<string>(),
                SubagentLabel: subagent?["label"]?.GetValue<string>(),
                // 标题在 projections.values.title —— 内核**顺手就给**了，
                // 不用为了拿标题去重载每一个会话。
                Title: values?["title"]?.GetValue<string>(),
                Raw: o));
        }

        return result;
    }

    /// <summary>
    /// 打开一个会话：读 opening 快照并把它折叠成视图，同时留下流继续读。
    ///
    /// <paramref name="withAssistantStream"/> 打开的话，内核还会推**进程内**的
    /// 助手流帧（没落盘的增量）。界面上要流式观感就得开；
    /// 做「实时 vs 重放」对账时要**关**，否则重放侧没有对应物。
    ///
    /// <paramref name="loadAllHistory"/> 打开的话，会用 <c>session/page</c>
    /// **往回翻到底**再折叠 —— 快照只是**一页**（实测约 2500 条记录），
    /// 拿一页去和整份日志比会得到一堆假差异（参 P13）。
    /// 界面上的「往上滚加载更早」要的也是这件事。
    /// </summary>
    public async Task<HarnessSessionStream> OpenAsync(
        HarnessAddress address,
        bool withAssistantStream = true,
        int maxMessages = 200,
        TimeSpan? openBudget = null,
        bool loadAllHistory = false,
        CancellationToken cancellationToken = default)
    {
        var sessionId = address.SessionId;

        // 与 CheckStreamRoutingAsync 共用同一份请求构造 —— 两处各写一遍的话，
        // 「探针验过的东西」与「产品真正发的请求」就有分叉的余地。
        var request = FollowRequest(address, withAssistantStream, maxMessages);

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
        var pages = new List<JsonArray>();
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

            // 快照这一页**先不折叠** —— 往回翻的页比它更早，而折叠必须按时间顺序。
            // 所以先把「从新到旧」的页攒起来，翻完再反着折一遍。
            pages.Add(value["records"] as JsonArray ?? new JsonArray());

            // 打开到此为止 —— 理由见上面那段注释里的上游契约。
            break;
        }

        // ── 往回翻到底（可选）────────────────────────────────────────
        //
        // 下一页的游标来自**本页第一条记录的 seq 减一**：上游的 `paginate`
        // 返回 `events.slice(cut, end)`，而 `cut` 就是第一条事件的 seq
        // （日志的 seq 是稠密零基前缀，所以位置就是 seq）。
        // 而且 `session/page` 会校验 `sourceLog[throughSeq].seq === throughSeq` ——
        // 差一个就报 `session log does not contain through seq N`，所以
        // **不能拿「上一页的最小 seq」直接当游标，必须减一**。
        while (loadAllHistory && hasMore && pages.Count < MaxHistoryPages)
        {
            var oldest = FirstRecordSeq(pages[^1]);
            if (oldest <= 0) break; // 到头了（throughSeq=-1 会回一个空页，见上游）

            var page = await PageAsync(address, oldest - 1, maxMessages, cancellationToken).ConfigureAwait(false);
            var pageOldest = FirstRecordSeq(page.Records);

            // **无进展就停。** 页取空了、或者游标没往回走，都说明再翻也不会前进 ——
            // 没有这道闸，一个不按契约递减的游标会让这里死循环。
            if (page.Records.Count == 0 || pageOldest < 0 || pageOldest >= oldest) break;

            pages.Add(page.Records);
            hasMore = page.HasMore;
        }

        // 从**最旧**的一页开始折，才是时间顺序。
        for (var i = pages.Count - 1; i >= 0; i--)
        {
            foreach (var record in pages[i])
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
            PagesLoaded = pages.Count,
            HistoryComplete = !hasMore,
        };
    }

    /// <summary>
    /// 往回翻的上限。上游的会话再长也就是几十轮对话，这个数字不该被撞到 ——
    /// 它存在的意义是「万一游标逻辑写错，是停住而不是转到天亮」。
    /// </summary>
    private const int MaxHistoryPages = 500;

    /// <summary>一页记录里**第一条**的 seq（两种 record 都用 <c>event</c> 这个键）。</summary>
    private static long FirstRecordSeq(JsonArray records)
    {
        if (records.Count == 0) return -1;
        return records[0]?["event"]?["seq"]?.GetValue<long>() ?? -1;
    }

    /// <summary>
    /// 读一页更早的历史（<c>session/page</c>）。
    ///
    /// 契约来自上游源码（`types/history.js` 的 `page`）：请求是
    /// <c>{ address, throughSeq, beforeSeq?, maxMessages? }</c>，
    /// 返回 <c>{ records, hasMore }</c>。
    ///
    /// **`throughSeq` 是必需字段**，且 <c>-1</c> 表示「到头了」（返回空页，
    /// 不是「整条日志」）—— 这一点反直觉，所以往回翻的循环靠
    /// <see cref="FirstRecordSeq"/> 递减而不是靠 -1。
    /// </summary>
    public async Task<HarnessHistoryPage> PageAsync(
        HarnessAddress address,
        long throughSeq,
        int maxMessages = 200,
        CancellationToken cancellationToken = default)
    {
        var request = new JsonObject
        {
            ["address"] = address.ToJson(),
            ["throughSeq"] = throughSeq,
            ["maxMessages"] = maxMessages,
        };

        // 形参名是 `request`（`page(request, signal)`）—— 与 `session/list` 的
        // `_request` 不是一个名字。Gateway 按描述符逐字校验字段集合。
        var value = await _rpc.CallAsync("session", "page",
            new JsonObject { ["request"] = request }, cancellationToken).ConfigureAwait(false);

        return new HarnessHistoryPage(
            value?["records"] as JsonArray ?? new JsonArray(),
            value?["hasMore"]?.GetValue<bool>() ?? false);
    }

    /// <summary>
    /// 试开一个会话、只读到快照为止，回报「拿到了谁的快照」或「内核报了什么」。
    ///
    /// 分流探针需要**两个真能打开的**会话，而挑会话不能只看时间 ——
    /// 实测按 `updatedAt` 取最新的两个，正好落在两份**损坏的**日志上
    /// （一份正在被写、一份 seq gap，参 P14），于是探针报「串帧了」，
    /// 而真实原因是「打不开」。**失败的断言指向错误的方向，比没有断言更糟**（参 U13/U17）。
    /// 所以先筛一遍，并把每一步的结果都报出来。
    /// </summary>
    public async Task<HarnessSnapshotProbe> ProbeSnapshotAsync(
        HarnessAddress address,
        CancellationToken cancellationToken = default)
    {
        var sessionId = address.SessionId;
        var stream = await _mux.OpenAsync("session/follow",
            new JsonObject { ["request"] = FollowRequest(address, false) }, cancellationToken).ConfigureAwait(false);

        using var budget = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        budget.CancelAfter(TimeSpan.FromSeconds(30));

        try
        {
            while (true)
            {
                var frame = await stream.NextAsync(budget.Token).ConfigureAwait(false);
                if (frame is null) return new HarnessSnapshotProbe(sessionId, false, null, "流在给出快照前就结束了");

                if (frame.Value.Kind == HarnessMux.MuxFrameKind.Error)
                {
                    // 只取第一行：内核的错误正文很长（带 details 的 JSON 形状），
                    // 整段塞进一行会把探针的输出淹掉。
                    var message = frame.Value.Error?.Message ?? "（错误帧没带消息）";
                    var newline = message.IndexOf('\n');
                    return new HarnessSnapshotProbe(sessionId, false, null,
                        newline < 0 ? message : message[..newline] + "…");
                }
                if (frame.Value.Kind != HarnessMux.MuxFrameKind.Item) continue;

                var value = frame.Value.Value as JsonObject;
                if (value?["type"]?.GetValue<string>() != "snapshot") continue;
                return new HarnessSnapshotProbe(sessionId, true, value["header"]?["id"]?.GetValue<string>(), null);
            }
        }
        catch (OperationCanceledException)
        {
            return new HarnessSnapshotProbe(sessionId, false, null, "30s 内没有给出快照");
        }
    }

    /// <summary>一次「试着打开」的结果。</summary>
    public sealed record HarnessSnapshotProbe(string Requested, bool Ok, string? SessionIdSeen, string? Error);

    /// <summary>
    /// 在**同一条** mux 连接上同时开两条流，检查每一条只收到自己的帧。
    ///
    /// 为什么需要这个探针：分流这件事在只有一条流时**完全看不出来** ——
    /// 共用一个队列也会「正常工作」。所以「按 streamId 分好了」这句话
    /// 只有同时开两条流才算被证明过（参 P9：论证可以写，别当成证据）。
    ///
    /// 判据用快照自带的 <c>header.id</c>：它是内核给的会话身份，
    /// 不是我们请求时填的那个串 —— 所以「A 流拿到了 B 的快照」会当场露出来，
    /// 而不是被我们自己的参数掩盖掉。
    /// </summary>
    public async Task<HarnessMuxCheck> CheckStreamRoutingAsync(
        HarnessAddress addressA,
        HarnessAddress addressB,
        CancellationToken cancellationToken = default)
    {
        var notes = new List<string>();
        var sessionA = addressA.SessionId;
        var sessionB = addressB.SessionId;

        var streamA = await _mux.OpenAsync("session/follow",
            new JsonObject { ["request"] = FollowRequest(addressA, false) }, cancellationToken).ConfigureAwait(false);
        var streamB = await _mux.OpenAsync("session/follow",
            new JsonObject { ["request"] = FollowRequest(addressB, false) }, cancellationToken).ConfigureAwait(false);

        notes.Add($"开了两条流：A={streamA.StreamId[..8]} B={streamB.StreamId[..8]}（同一条 WS）");

        // **同时**等两条流，而不是 A 读完再读 B：串帧的要害正是并发 ——
        // 顺序读会让「共用一个队列」也有很大机会蒙对。
        var readA = ReadSnapshotSessionIdAsync(streamA, cancellationToken);
        var readB = ReadSnapshotSessionIdAsync(streamB, cancellationToken);
        await Task.WhenAll(readA, readB).ConfigureAwait(false);

        var idA = readA.Result;
        var idB = readB.Result;

        notes.Add($"A 的快照 header.id={idA ?? "(没读到快照)"}");
        notes.Add($"B 的快照 header.id={idB ?? "(没读到快照)"}");

        var ok = idA == sessionA && idB == sessionB;
        return new HarnessMuxCheck(ok, idA, idB, notes);
    }

    /// <summary>两条流各自的路由检查结果。</summary>
    public sealed record HarnessMuxCheck(bool Ok, string? SessionIdSeenByA, string? SessionIdSeenByB, List<string> Notes);

    /// <summary>
    /// `session/follow` 的请求体。`follow(request, signal)` → wire 字段是 `request`。
    /// </summary>
    private static JsonObject FollowRequest(HarnessAddress address, bool withAssistantStream, int maxMessages = 200)
    {
        var request = new JsonObject
        {
            ["address"] = address.ToJson(),
            ["maxMessages"] = maxMessages,
        };
        if (withAssistantStream) request["assistantStream"] = true;
        return request;
    }

    /// <summary>读到 <c>snapshot</c> 帧并回它的 <c>header.id</c>；读不到返回 null。</summary>
    private static async Task<string?> ReadSnapshotSessionIdAsync(HarnessStream stream, CancellationToken cancellationToken)
    {
        using var budget = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        budget.CancelAfter(TimeSpan.FromSeconds(30));

        try
        {
            while (true)
            {
                var frame = await stream.NextAsync(budget.Token).ConfigureAwait(false);
                if (frame is null) return null;
                if (frame.Value.Kind != HarnessMux.MuxFrameKind.Item) continue;

                var value = frame.Value.Value as JsonObject;
                if (value?["type"]?.GetValue<string>() != "snapshot") continue;
                return value["header"]?["id"]?.GetValue<string>();
            }
        }
        catch (OperationCanceledException)
        {
            return null;
        }
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
