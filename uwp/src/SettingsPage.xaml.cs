using System;
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
            VersionText.Text = $"Arc Deck UWP · {v.Major}.{v.Minor}.{v.Build}";

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

            // 主题挂在 Window.Content 上而不是本页：这样连导航栏一起换，
            // 不会出现「内容变亮、侧栏还是暗的」。
            Settings.ApplyTheme(Window.Current.Content as FrameworkElement);
        }
    }
}
