using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;

namespace ArcDeck
{
    /// <summary>设置页。</summary>
    public sealed partial class SettingsPage : Page
    {
        public SettingsPage()
        {
            InitializeComponent();

            var v = Windows.ApplicationModel.Package.Current.Id.Version;
            VersionText.Text = $"Arc Deck UWP · {v.Major}.{v.Minor}.{v.Build}";
        }

        /// <summary>
        /// 主题切换。
        ///
        /// 「跟随系统」是 UWP 的默认行为，也是该有的行为 —— 应用不该在这里塞
        /// 自己的配色。主题、强调色、圆角全部由 Windows 决定。
        /// </summary>
        private void OnThemeChecked(object sender, RoutedEventArgs e)
        {
            if (Window.Current.Content is not FrameworkElement root
                || sender is not RadioButton rb
                || rb.Tag is not string tag)
            {
                return;
            }

            root.RequestedTheme = tag switch
            {
                "light" => ElementTheme.Light,
                "dark" => ElementTheme.Dark,
                _ => ElementTheme.Default,
            };
        }
    }
}
