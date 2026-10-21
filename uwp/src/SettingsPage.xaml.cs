using System;
using System.Text;
using System.Threading.Tasks;
using ArcDeck.Services;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;

namespace ArcDeck
{
    /// <summary>设置页。</summary>
    public sealed partial class SettingsPage : Page
    {
        /// <summary>初始化期间抑制 Checked 处理 —— 否则回填单选按钮会立刻回写一次设置。</summary>
        bool _loading;

        public SettingsPage()
        {
            InitializeComponent();

            var v = Windows.ApplicationModel.Package.Current.Id.Version;
            VersionText.Text = $"Saiku Deck UWP · {v.Major}.{v.Minor}.{v.Build}";

            LoadSettings();
        }

        private void LoadSettings()
        {
            _loading = true;

            switch (Settings.Theme)
            {
                case AppTheme.Light:
                    ThemeLight.IsChecked = true;
                    break;
                case AppTheme.Dark:
                    ThemeDark.IsChecked = true;
                    break;
                default:
                    ThemeSystem.IsChecked = true;
                    break;
            }

            _loading = false;
        }

        /// <summary>
        /// 拉起伴随进程并跑一遍最小检查：ping / env / probe。
        ///
        /// 这不是装饰：`probe` 会真的起一个 `dsh --version` 子进程，而「在 AppContainer
        /// 里做不到」正是整条通道存在的理由。检查结果里既有 `env` 的 %APPDATA% 路径，
        /// 也有子进程的真实退出码 —— 任何一半坏了这里都看得见。
        /// </summary>
        private async void OnHostCheck(object sender, RoutedEventArgs e)
        {
            HostCheckButton.IsEnabled = false;
            HostResultText.Text = "";
            try
            {
                HostStateText.Text = await HostBridge.LaunchAsync();
                var connected = await HostBridge.WaitForConnectionAsync(TimeSpan.FromSeconds(8));
                HostStateText.Text = HostBridge.State;
                if (!connected)
                {
                    HostResultText.Text =
                        "伴随进程没有连上来。\n" +
                        "UWP 侧日志：" + HostBridge.LogPath + "\n" +
                        "进程侧日志：%TEMP%\\arcdeck-host.log";
                    return;
                }

                var builder = new StringBuilder();
                foreach (var op in new[] { "ping", "env", "probe" })
                {
                    var reply = await HostBridge.RequestAsync(op);
                    builder.AppendLine(op + " → " + reply.Display);
                }
                HostResultText.Text = builder.ToString();
            }
            catch (Exception ex)
            {
                HostResultText.Text = "检查失败：" + ex.GetType().Name + " " + ex.Message;
            }
            finally
            {
                HostCheckButton.IsEnabled = true;
            }
        }

        /// <summary>
        /// 主题切换。
        ///
        /// 改动分两步，缺一不可：**存下来**，以及**立刻应用**。
        /// 只应用不存，重开就回到跟随系统；只存不应用，勾了看不到变化 ——
        /// 后者正是这一页原先的状态，看起来能配、实际什么都没发生。
        /// </summary>
        private void OnThemeChecked(object sender, RoutedEventArgs e)
        {
            if (_loading)
            {
                return;
            }

            if (sender is not RadioButton rb || rb.Tag is not string tag)
            {
                return;
            }

            Settings.Theme = tag switch
            {
                "light" => AppTheme.Light,
                "dark" => AppTheme.Dark,
                _ => AppTheme.System,
            };

            // 立即应用。定位到外壳而不是直接改本页 —— 本页是 Frame 的子级。
            //
            // 注意：这**不能**让 NavigationView 的窗格背景跟着换（那是平台在
            // 加载期解析的 AcrylicBrush）。见 ShellPage.ReloadTheme 的说明与
            // AGENTS.md U9。内容区会正确变色，窗格保持平台原样。
            if ((Window.Current.Content as Frame)?.Content is ShellPage shell)
            {
                shell.ReloadTheme();
            }
            else
            {
                Settings.ApplyTheme(Window.Current.Content as FrameworkElement);
            }
        }
    }
}
