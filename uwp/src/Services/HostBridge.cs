using System;
using System.IO;
using System.Text;
using System.Threading.Tasks;
using Windows.ApplicationModel;
using Windows.ApplicationModel.AppService;
using Windows.Foundation.Collections;
using Windows.Storage;

namespace ArcDeck.Services
{
    /// <summary>伴随进程的一次应答。`Data` 是它给的 JSON 原文，界面自己决定怎么显示。</summary>
    public sealed class HostReply
    {
        public bool Ok { get; set; }
        public string Data { get; set; }
        public string Error { get; set; }

        public string Display => Ok ? Data : "错误：" + Error;
    }

    /// <summary>
    /// UWP 与完整信任伴随进程之间的唯一通道。
    ///
    /// 为什么需要它：UWP 是 AppContainer，起不了 `dsh` 子进程、读不到 `%APPDATA%`
    /// （AGENTS P1 的三条结构性限制，改代码绕不过去）。完整信任的那一半负责执行与文件访问，
    /// 界面只消费它的应答。
    ///
    /// 方向：UWP 是 AppService 的 **provider**（服务必须由包自己声明），伴随进程是 **client**，
    /// 它用自己的包身份 `OpenAsync` 连上来；之后界面用 `SendMessageAsync` 发请求，它应答。
    /// 别把它写反 —— 完整信任进程不能当 provider。
    /// </summary>
    public static class HostBridge
    {
        const string ServiceName = "arcdeck.host";

        static AppServiceConnection _connection;
        static int _sequence;
        static TaskCompletionSource<bool> _waiting;

        public static event EventHandler Changed;

        /// <summary>人话状态，设置页直接显示。连不上时这里就是排查入口。</summary>
        public static string State { get; private set; } = "未拉起";

        public static bool IsConnected => _connection != null;

        public static string LogPath
        {
            get
            {
                try { return Path.Combine(ApplicationData.Current.LocalFolder.Path, "host.log"); }
                catch { return "（拿不到 LocalFolder）"; }
            }
        }

        public static void Log(string line)
        {
            try
            {
                File.AppendAllText(LogPath, $"{DateTimeOffset.Now:HH:mm:ss.fff} {line}{Environment.NewLine}");
            }
            catch
            {
                // 日志失败不能拖垮界面。
            }
        }

        /// <summary>Windows 在 `OnBackgroundActivated` 里把这个连接交给我们。</summary>
        public static void Attach(AppServiceConnection connection)
        {
            _connection = connection;
            State = "已连接";
            Log("伴随进程已连接");
            _waiting?.TrySetResult(true);
            Changed?.Invoke(null, EventArgs.Empty);
        }

        /// <summary>
        /// 拉起伴随进程。它启动后自己连回来，所以这里只负责“叫醒”，不等待连接；
        /// 调用方用 <see cref="WaitForConnectionAsync"/> 等。
        /// </summary>
        public static async Task<string> LaunchAsync()
        {
            if (IsConnected)
            {
                State = "已连接";
                return State;
            }

            try
            {
                State = "正在拉起伴随进程…";
                Log("FullTrustProcessLauncher...");
                await FullTrustProcessLauncher.LaunchFullTrustProcessForCurrentAppAsync();
                State = "已拉起，等待它连上来…";
                Log("已请求启动");
            }
            catch (Exception ex)
            {
                State = "拉起失败：" + ex.GetType().Name + " " + ex.Message;
                Log("拉起失败：" + ex);
            }

            Changed?.Invoke(null, EventArgs.Empty);
            return State;
        }

        /// <summary>等伴随进程连上来。超时不是异常，`State` 会说明停在哪一步。</summary>
        public static async Task<bool> WaitForConnectionAsync(TimeSpan timeout)
        {
            if (IsConnected)
            {
                return true;
            }

            _waiting = new TaskCompletionSource<bool>();
            var finished = await Task.WhenAny(_waiting.Task, Task.Delay(timeout));
            if (finished != _waiting.Task || !IsConnected)
            {
                State = "等待连接超时（日志：" + LogPath + "）";
                return false;
            }

            return true;
        }

        /// <summary>发一个请求。没有连接时直接给出原因，不假装成功。</summary>
        public static async Task<HostReply> RequestAsync(string op, string payload = null)
        {
            var connection = _connection;
            if (connection == null)
            {
                return new HostReply { Ok = false, Error = "伴随进程未连接" };
            }

            var id = (++_sequence).ToString();
            var message = new ValueSet { ["id"] = id, ["op"] = op };
            if (!string.IsNullOrEmpty(payload))
            {
                message["payload"] = payload;
            }

            AppServiceResponse response;
            try
            {
                response = await connection.SendMessageAsync(message);
            }
            catch (Exception ex)
            {
                Log($"op={op} 发送失败：{ex}");
                return new HostReply { Ok = false, Error = ex.Message };
            }

            if (response.Status != AppServiceResponseStatus.Success)
            {
                Log($"op={op} 失败：{response.Status}");
                return new HostReply { Ok = false, Error = "AppService：" + response.Status };
            }

            var reply = new HostReply
            {
                Ok = response.Message.TryGetValue("ok", out var ok) && ok is bool flag && flag,
                Data = response.Message.TryGetValue("data", out var data) ? data as string : null,
                Error = response.Message.TryGetValue("error", out var error) ? error as string : null,
            };
            Log($"op={op} → {(reply.Ok ? "ok" : "失败")}");
            return reply;
        }
    }
}
