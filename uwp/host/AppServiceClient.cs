using System.Text.Json;
using Windows.ApplicationModel.AppService;
using Windows.Foundation.Collections;

namespace ArcDeck.Host;

/// <summary>
/// The AppService half. Direction matters and is easy to get backwards:
///
///   UWP (provider, in-process background activation)  ←AppService→  this process (client)
///
/// The UWP app is the service provider because an app service must be declared by the package
/// that owns it, and a full-trust process cannot be one. This process opens the connection to
/// its own package, then answers requests the UWP side sends with SendMessageAsync.
///
/// The wire format is plain ValueSet keys (`id` / `op` / `payload` in, `ok` / `data` / `error`
/// out) instead of one JSON blob: the UWP side must stay on `Windows.Data.Json`, and
/// `System.Text.Json` is not part of its framework.
/// </summary>
internal static class AppServiceClient
{
    public static async Task<int> RunAsync(string serviceName)
    {
        string family;
        try
        {
            family = Windows.ApplicationModel.Package.Current.Id.FamilyName;
        }
        catch (Exception ex)
        {
            HostLog.Write($"没有包身份，无法连 AppService：{ex.GetType().Name}: {ex.Message}");
            return 4;
        }

        var connection = new AppServiceConnection
        {
            AppServiceName = serviceName,
            PackageFamilyName = family,
        };
        connection.RequestReceived += (sender, args) =>
        {
            var deferral = args.GetDeferral();
            _ = Task.Run(async () =>
            {
                try
                {
                    var reply = await HandleAsync(args.Request.Message).ConfigureAwait(false);
                    var response = new ValueSet
                    {
                        ["id"] = reply.Id ?? "",
                        ["ok"] = reply.Ok,
                        ["data"] = reply.Data ?? "",
                        ["error"] = reply.Error ?? "",
                    };
                    await args.Request.SendResponseAsync(response).AsTask().ConfigureAwait(false);
                }
                catch (Exception ex)
                {
                    HostLog.Write("处理请求失败：" + ex);
                }
                finally
                {
                    deferral.Complete();
                }
            });
        };
        connection.ServiceClosed += (sender, args) =>
        {
            HostLog.Write("服务关闭：" + args.Status);
            // 界面没了就退出：留一个守着已关闭连接的进程，下次装包会被它锁住文件
            // （0x80073D02「资源正在使用中」正是这么来的）。
            Environment.Exit(0);
        };

        HostLog.Write($"打开 AppService `{serviceName}`（{family}）");
        var status = await connection.OpenAsync();
        HostLog.Write("OpenAsync → " + status);
        if (status != AppServiceConnectionStatus.Success)
        {
            return 5;
        }

        // Stay alive for as long as the service connection exists. The UWP side closing the
        // connection ends the process; a packaged full-trust process must not linger.
        await Task.Delay(Timeout.Infinite).ConfigureAwait(false);
        return 0;
    }

    private static async Task<Reply> HandleAsync(ValueSet message)
    {
        var id = String(message, "id");
        var op = String(message, "op");
        if (string.IsNullOrWhiteSpace(op))
        {
            return Reply.Failure(id, "缺少 op");
        }
        var request = new Request
        {
            Id = id,
            Op = op,
            Payload = Payload(message),
        };
        try
        {
            var data = await Ops.RunAsync(request).ConfigureAwait(false);
            HostLog.Write($"op={op} ok");
            return Reply.Success(id, data);
        }
        catch (Exception ex)
        {
            HostLog.Write($"op={op} 失败：{ex.Message}");
            return Reply.Failure(id, ex.Message);
        }
    }

    private static string? String(ValueSet message, string key) =>
        message.TryGetValue(key, out var value) ? value as string : null;

    private static JsonElement? Payload(ValueSet message)
    {
        var text = String(message, "payload");
        if (string.IsNullOrWhiteSpace(text))
        {
            return null;
        }
        try
        {
            return JsonSerializer.Deserialize<JsonElement>(text);
        }
        catch
        {
            return null;
        }
    }
}
