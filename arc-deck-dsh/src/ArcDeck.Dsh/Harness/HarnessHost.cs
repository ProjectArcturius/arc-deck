using System.Diagnostics;
using System.Text;
using System.Text.RegularExpressions;

namespace ArcDeck.Dsh.Harness;

/// <summary>
/// 一个**正在运行的** harness web 内核，连同它的登录票据。
///
/// ## 为什么这条路与 SDK stdio 并存，而不是取代它
///
/// 两条路给的东西不一样，而且都是真的：
///
/// | | SDK stdio（<c>dsh --profile sdk</c>） | harness web（<c>dsh web</c>） |
/// |---|---|---|
/// | 会话列表 | ❌ 只能自己扫磁盘 | ✅ <c>session/list</c>，带 running 与 cwd |
/// | 历史 | 只有自己写出来的那一段 | ✅ <c>session/page</c> + <c>session/follow</c> 实时 |
/// | 改名 / 取消 / 排队 | ❌ 协议里没有 | ✅ <c>session/rename</c> / <c>cancel</c> / <c>updateQueue</c> |
/// | 需要的外部件 | 只要内核 | 还要一份 web profile（DSH Desktop 装的那份即可） |
///
/// 完整功能面在 web 这一侧 —— 它正是 DSH Desktop 那个 WebView 客户端用的那套
/// （`/api` HTTP + `/api/remote.mux` WebSocket）。所以「接真实 harness」以这条为主。
///
/// ## 票据是怎么拿到的
///
/// 每个进程生成一个**随机启动令牌**，内核把它连同一个 URL 打到 stdout。
/// 换票只有**一个**口子：<c>GET /?token=…</c>，它回一个绑定 authority 的签名
/// cookie 并 303 到干净的 <c>/</c>。之后所有 RPC 与 WebSocket 都要带这个 cookie。
///
/// 实测确认（`tools/harness-probe`）：
/// <list type="bullet">
///   <item>从 stdout 正则抓到 <c>http://127.0.0.1:&lt;port&gt;/?token=…</c></item>
///   <item>GET 该 URL → 303 到 <c>/</c>，Set-Cookie <c>dsh-auth-…=v1.…</c></item>
///   <item>带该 cookie 再 GET <c>/</c> → 200（换票成功）</item>
/// </list>
/// </summary>
public sealed class HarnessHost : IAsyncDisposable
{
    /// <summary>stdout 上那一行带票的 URL —— 内核自己决定打印的格式，我们只认形状。</summary>
    private static readonly Regex TokenUrlPattern = new(
        @"https?://127\.0\.0\.1:\d+/\?token=[A-Za-z0-9_\-]+",
        RegexOptions.Compiled);

    private readonly Process _process;
    private readonly StringBuilder _stdout = new();

    private HarnessHost(Process process, string origin, string tokenUrl, string cookie)
    {
        _process = process;
        Origin = origin;
        TokenUrl = tokenUrl;
        AuthCookie = cookie;
    }

    /// <summary>内核的 HTTP origin，例如 <c>http://127.0.0.1:51058</c>。</summary>
    public string Origin { get; }

    /// <summary>
    /// 带票的根 URL。
    ///
    /// **它是一个凭据**：拿着它的人可以换到会话 cookie。所以只用来换票，
    /// 不要写进日志或错误消息（<see cref="ToString"/> 刻意不包含它）。
    /// </summary>
    public string TokenUrl { get; }

    /// <summary>换票得到的浏览器会话 cookie。所有 /api 请求与 WebSocket 都要带它。</summary>
    public string AuthCookie { get; }

    public override string ToString() => $"harness web @ {Origin}（已登录）";

    /// <summary>
    /// 起一个 harness web 内核并登录。
    ///
    /// <paramref name="port"/> 传 0 表示让内核自己挑 —— 那时端口要从 stdout 的
    /// URL 里读回来，所以这个参数其实只是「尽量用它」。
    /// </summary>
    public static async Task<HarnessHost?> StartAsync(
        HarnessInstall install,
        int port = 0,
        Action<string>? diagnostic = null,
        CancellationToken cancellationToken = default)
    {
        var psi = new ProcessStartInfo(install.NodeExecutable)
        {
            // `web --no-open` —— **必须带 --no-open**，否则内核会去拉起用户的
            // 默认浏览器，而那正是我们要取代的那个界面。
            Arguments = $"\"{install.DshEntry}\" web --no-open --host 127.0.0.1 --port {port}",
            WorkingDirectory = install.HarnessHome,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            StandardOutputEncoding = new UTF8Encoding(false),
            StandardErrorEncoding = new UTF8Encoding(false),
        };
        psi.Environment["DSH_HOME"] = install.HarnessHome;

        var process = Process.Start(psi)!;
        var host = new HarnessHost(process, string.Empty, string.Empty, string.Empty);

        // URL 可能出现在 stdout 或 stderr 上（实测两处都见过）。
        var urlTask = new TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously);
        void Scan(string chunk)
        {
            host._stdout.Append(chunk);
            if (urlTask.Task.IsCompleted) return;
            var m = TokenUrlPattern.Match(host._stdout.ToString());
            if (m.Success) urlTask.TrySetResult(m.Value);
        }

        // **两个管道必须一直读到底。**
        //
        // 实测踩到过：登录成功之后不再读 stderr，于是内核往 stderr 写满那个
        // 64 KB 管道缓冲后**阻塞在写调用上** —— 表现为 `session/follow` 永远不
        // 给出快照，而 stdout 上一条错都没有。这类「对端卡在写诊断信息上」
        // 的停滞最难查，因为看起来像我们自己的读循环坏了。
        process.OutputDataReceived += (_, e) => { if (e.Data is not null) Scan(e.Data); };
        process.ErrorDataReceived += (_, e) =>
        {
            if (e.Data is null) return;
            Scan(e.Data);
            if (!urlTask.Task.IsCompleted) diagnostic?.Invoke($"[harness] {e.Data}");
        };
        process.BeginOutputReadLine();
        process.BeginErrorReadLine();

        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(TimeSpan.FromSeconds(120));

        string tokenUrl;
        try
        {
            tokenUrl = await urlTask.Task.WaitAsync(timeout.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            diagnostic?.Invoke($"[harness] 等带票 URL 超时；stdout 尾部：{Tail(host._stdout.ToString())}");
            await host.DisposeAsync().ConfigureAwait(false);
            return null;
        }

        // 换票。**这是唯一的登录口** —— 内核不在别处接受 token
        // （不接受 Authorization 头、也不接受其他路径上的 query token）。
        //
        // **不自动跟随重定向**：换票是 `GET /?token=…` → **303** → `GET /`，
        // 而 `Set-Cookie` 挂在 303 那一跳上。跟随之后拿到的最终响应里没有它 ——
        // 表现成「HTTP 200 但没拿到 cookie」（实测第一次就是这样）。
        var origin = new Uri(tokenUrl).GetLeftPart(UriPartial.Authority);
        var handler = new SocketsHttpHandler { AllowAutoRedirect = false, UseCookies = false };
        string? cookie = null;
        System.Net.HttpStatusCode status;
        using (var http = new HttpClient(handler) { Timeout = TimeSpan.FromSeconds(30) })
        {
            using var exchange = await http.GetAsync(tokenUrl, cancellationToken).ConfigureAwait(false);
            status = exchange.StatusCode;
            if (exchange.Headers.TryGetValues("Set-Cookie", out var values))
            {
                // 只取 `name=value` 那一段，属性（Path/HttpOnly/SameSite/过期）不要 ——
                // 自己拼请求头时属性会变成非法 token。
                cookie = string.Join("; ", values.Select(v => v.Split(';')[0]));
            }
        }

        if (string.IsNullOrEmpty(cookie))
        {
            diagnostic?.Invoke($"[harness] 换票没有拿到 Set-Cookie（HTTP {(int)status}）—— " +
                               "token 只对根路径有效，且只用一次；如果内核是自己刚起的，这不该发生");
            await host.DisposeAsync().ConfigureAwait(false);
            return null;
        }

        var logged = new HarnessHost(process, origin, tokenUrl, cookie);
        diagnostic?.Invoke($"[harness] 已登录 {origin}（HTTP {(int)status}，cookie 长度 {cookie.Length}）");
        return logged;
    }

    internal Process Process => _process;

    private static string Tail(string s) => s.Length <= 800 ? s : "…" + s[^800..];

    public async ValueTask DisposeAsync()
    {
        try
        {
            if (!_process.HasExited)
            {
                // 内核自己没有「优雅停机」的 HTTP 接口 —— 它归 app bin 管（EOF 与信号）。
                // 这里直接杀进程树：它会按 close 路径收尾，会话已经按事件落盘。
                _process.Kill(entireProcessTree: true);
                await _process.WaitForExitAsync().ConfigureAwait(false);
            }
        }
        catch { }
        finally { _process.Dispose(); }
    }
}
