using System;
using System.Text;
using Windows.ApplicationModel;
using Windows.ApplicationModel.Activation;
using Windows.Storage;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Navigation;

namespace ArcDeck
{
    /// <summary>
    /// 应用入口。
    ///
    /// 外观交给系统：主题、强调色、圆角、动效全部跟 Windows 走。UWP 应用不该
    /// 在这里重新定义配色 —— 用户换了系统主题，应用就该跟着变。
    /// </summary>
    sealed partial class App : Application
    {
        public App()
        {
            InitializeComponent();

            // 启动期异常落盘。
            //
            // 为什么需要：UWP 的未处理异常默认只进事件日志，而事件日志里的
            // .NET Runtime 条目**不含异常消息**（只有代号）。一个 XAML 资源键
            // 写错就会崩，没有消息根本不知道是哪一处。
            UnhandledException += (_, e) => WriteCrashLog(e.Exception);
            Suspending += OnSuspending;
        }

        internal static void WriteCrashLog(Exception ex)
        {
            try
            {
                var sb = new StringBuilder();
                sb.AppendLine($"--- {DateTimeOffset.Now:O} ---");
                sb.AppendLine($"{ex?.GetType().FullName}: {ex?.Message}");

                var inner = ex?.InnerException;
                for (var i = 0; inner != null && i < 6; i++)
                {
                    sb.AppendLine($"  [inner {i}] {inner.GetType().Name}: {inner.Message}");
                    inner = inner.InnerException;
                }

                sb.AppendLine(ex?.StackTrace);
                sb.AppendLine();

                var path = System.IO.Path.Combine(
                    ApplicationData.Current.LocalFolder.Path, "crash.log");
                System.IO.File.AppendAllText(path, sb.ToString());
            }
            catch
            {
                // 崩溃日志本身绝不能再抛 —— 那会把「可诊断的失败」变成「不可诊断的失败」。
            }
        }

        /// <summary>
        /// 伴随进程连上来时 Windows 从这里把 AppService 连接交给我们。in-proc app service
        /// 的激活会落到正在运行的实例上，所以界面与这个连接在同一个进程里。
        ///
        /// deferral 要一直握着：它是一个长连接，不是一次性的后台任务。挂起时在
        /// <see cref="OnSuspending"/> 里完成它。
        /// </summary>
        protected override void OnBackgroundActivated(BackgroundActivatedEventArgs args)
        {
            if (args.TaskInstance.TriggerDetails is Windows.ApplicationModel.AppService.AppServiceTriggerDetails details)
            {
                // App Service 不是「只有本包能连」的：任何知道包族名与服务名的应用都能尝试。
                // 这个服务能起子进程、读包外路径，必须先确认调用方就是本包。
                var caller = details.CallerPackageFamilyName;
                var self = Package.Current.Id.FamilyName;
                if (!string.Equals(caller, self, StringComparison.OrdinalIgnoreCase))
                {
                    WriteCrashLog(new Exception($"拒绝 AppService 连接：调用方 {caller} 不是 {self}"));
                    base.OnBackgroundActivated(args);
                    return;
                }

                _serviceDeferral = args.TaskInstance.GetDeferral();
                Services.HostBridge.Attach(details.AppServiceConnection);
            }
            base.OnBackgroundActivated(args);
        }

        Windows.ApplicationModel.Background.BackgroundTaskDeferral _serviceDeferral;

        protected override void OnLaunched(LaunchActivatedEventArgs e)
        {
            var rootFrame = Window.Current.Content as Frame;

            if (rootFrame == null)
            {
                rootFrame = new Frame();
                rootFrame.NavigationFailed += OnNavigationFailed;
                Window.Current.Content = rootFrame;
            }

            if (!e.PrelaunchActivated && rootFrame.Content == null)
            {
                rootFrame.Navigate(typeof(ShellPage), e.Arguments);
            }

            // 主题由 ShellPage 自己应用（挂在 NavigationView 上）。
            // 这里不再重复设置 rootFrame —— Frame 只是 NavigationView 的子级，
            // 挂在父级上才能连导航窗格一起换。

            Window.Current.Activate();
        }

        private void OnNavigationFailed(object sender, NavigationFailedEventArgs e)
        {
            WriteCrashLog(new Exception($"无法导航到页面 {e.SourcePageType?.FullName}", e.Exception));
            throw new Exception($"无法导航到页面 {e.SourcePageType?.FullName}");
        }

        private void OnSuspending(object sender, SuspendingEventArgs e)
        {
            var deferral = e.SuspendingOperation.GetDeferral();
            _serviceDeferral?.Complete();
            _serviceDeferral = null;
            deferral.Complete();
        }
    }
}
