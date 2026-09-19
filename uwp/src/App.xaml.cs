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
            deferral.Complete();
        }
    }
}
