using System.Net;
using System.Net.Http.Json;
using System.Text.Json.Nodes;

namespace ArcDeck.Dsh.Harness;

/// <summary>
/// Remote 层的一元调用：<c>POST /api/&lt;namespace&gt;/&lt;method&gt;</c>。
///
/// ## 协议形状（从 harness 源码读出，并在 0.1.6 上实测过）
///
/// 请求体是一个 **Connection 信封**，不是裸参数：
///
/// <code>
/// { "type": "client-request", "rpcId": "&lt;uuid&gt;", "method": "&lt;ns&gt;/&lt;method&gt;",
///   "payload": { "args": { … } } }
/// </code>
///
/// 响应体是 <c>{ type:"server-response", rpcId, result }</c>，
/// 其中 <c>result</c> 是 <c>{ ok:true, value }</c> 或
/// <c>{ ok:false, error:{ code, message, details } }</c>。
///
/// ## 两个必须照抄的约束
///
/// 1. **<c>args</c> 的字段名要与 Host 形参逐字一致。** 实测第一次用空对象调
///    <c>session/list</c>，被拒为
///    「args fields do not match the descriptor: missing "_request"」——
///    Gateway 要求字段集合**完全一致**（多一个少一个都报错）。
///    所以 <c>session/list</c> 传 <c>{ "_request": {} }</c>，
///    <c>session/prompt</c> 传 <c>{ "request": {…} }</c>，两者不一样。
/// 2. **<c>cancellation</c> 参数不进 args。** <c>signal</c> 记在描述符里，
///    所以别把它写进去。
/// </summary>
public sealed class HarnessRpc : IDisposable
{
    private readonly HttpClient _http;

    public HarnessRpc(HarnessHost host)
    {
        // Cookie 放在默认头上，而不是每次拼 —— 漏一处就是 401，
        // 而 401 的表现是「什么都没有」，不指向 cookie。
        var handler = new HttpClientHandler { UseCookies = false, AllowAutoRedirect = false };
        _http = new HttpClient(handler) { BaseAddress = new Uri(host.Origin), Timeout = TimeSpan.FromMinutes(5) };
        _http.DefaultRequestHeaders.Add("Cookie", host.AuthCookie);
    }

    /// <summary>调用一个 Remote 一元方法；失败时抛 <see cref="HarnessRpcException"/>。</summary>
    public async Task<JsonNode?> CallAsync(
        string ns,
        string method,
        JsonObject args,
        CancellationToken cancellationToken = default)
    {
        var rpcId = Guid.NewGuid().ToString();
        var envelope = new JsonObject
        {
            ["type"] = "client-request",
            ["rpcId"] = rpcId,
            ["method"] = $"{ns}/{method}",
            ["payload"] = new JsonObject { ["args"] = args },
        };

        using var response = await _http.PostAsJsonAsync($"/api/{ns}/{method}", envelope, cancellationToken)
            .ConfigureAwait(false);

        if (response.StatusCode is HttpStatusCode.Unauthorized)
        {
            throw new HarnessRpcException("unauthenticated",
                "内核回了 401 —— 浏览器会话 cookie 无效或已过期（删掉 .credentials.yaml 里的 grant 记录会让它立刻失效）", null);
        }

        if (!response.IsSuccessStatusCode)
        {
            throw new HarnessRpcException($"http-{(int)response.StatusCode}",
                $"{ns}/{method} 返回 HTTP {(int)response.StatusCode}", null);
        }

        var frame = await response.Content.ReadFromJsonAsync<JsonNode>(cancellationToken).ConfigureAwait(false);
        if (frame is not JsonObject obj)
        {
            throw new HarnessRpcException("bad-envelope", $"{ns}/{method} 的响应不是 JSON 对象", null);
        }

        if (obj["rpcId"]?.GetValue<string>() != rpcId)
        {
            throw new HarnessRpcException("rpc-id-mismatch",
                $"{ns}/{method} 的 rpcId 不匹配", null);
        }

        var result = obj["result"] as JsonObject
            ?? throw new HarnessRpcException("bad-envelope", $"{ns}/{method} 的 result 缺失", null);

        if (result["ok"]?.GetValue<bool>() == true)
        {
            return result["value"];
        }

        var error = result["error"] as JsonObject;
        throw new HarnessRpcException(
            error?["code"]?.GetValue<string>() ?? "unknown",
            error?["message"]?.GetValue<string>() ?? "（无消息）",
            error?["details"]);
    }

    public void Dispose() => _http.Dispose();
}

public sealed class HarnessRpcException : Exception
{
    public string Code { get; }
    public JsonNode? Details { get; }

    public HarnessRpcException(string code, string message, JsonNode? details) : base($"[{code}] {message}")
    {
        Code = code;
        Details = details;
    }
}
