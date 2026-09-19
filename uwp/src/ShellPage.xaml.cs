using System;
using System.Collections.ObjectModel;
using System.Linq;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Media.Animation;

namespace ArcDeck
{
    /// <summary>
    /// 应用外壳：NavigationView + Frame。
    ///
    /// 导航用 UWP 的 <see cref="Frame"/> 而不是自己维护页面栈 —— 这是平台做法，
    /// 免费拿到返回栈、页面缓存与转场。UWP 应用不需要重新发明这一层。
    /// </summary>
    public sealed partial class ShellPage : Page
    {
        /// <summary>导航项。用 NavigationViewItem 直接构造。</summary>
        public ObservableCollection<Microsoft.UI.Xaml.Controls.NavigationViewItem> MenuItems { get; }
            = new ObservableCollection<Microsoft.UI.Xaml.Controls.NavigationViewItem>();

        /// <summary>窗格模式断点（DIP）。数值取 UWP 的惯用档位。</summary>
        private const double WideThreshold = 1008;
        private const double MediumThreshold = 641;

        public ShellPage()
        {
            BuildMenuItems();
            InitializeComponent();

            Nav.SelectedItem = MenuItems[0];
        }

        private void BuildMenuItems()
        {
            MenuItems.Add(Item("会话", "Message", "sessions"));
            MenuItems.Add(Item("工作区", "Folder", "workspaces"));
            MenuItems.Add(Item("工具", "Repair", "tools"));
        }

        private static Microsoft.UI.Xaml.Controls.NavigationViewItem Item(string text, string glyph, string tag)
            => new Microsoft.UI.Xaml.Controls.NavigationViewItem
            {
                Content = text,
                Tag = tag,
                Icon = new SymbolIcon((Symbol)Enum.Parse(typeof(Symbol), glyph)),
            };

        /// <summary>
        /// 自适应窗格模式 —— UWP 应用的标准行为。
        ///
        /// 宽屏展开、中屏只留图标、窄屏不显示窗格。用户拖窗口时跟着变，
        /// 而不是写死一种模式。
        /// </summary>
        private void OnNavSizeChanged(object sender, SizeChangedEventArgs e)
        {
            var w = e.NewSize.Width;

            if (w >= WideThreshold)
            {
                Nav.PaneDisplayMode = Microsoft.UI.Xaml.Controls.NavigationViewPaneDisplayMode.Left;
                Nav.IsPaneOpen = true;
            }
            else if (w >= MediumThreshold)
            {
                Nav.PaneDisplayMode = Microsoft.UI.Xaml.Controls.NavigationViewPaneDisplayMode.LeftCompact;
            }
            else
            {
                Nav.PaneDisplayMode = Microsoft.UI.Xaml.Controls.NavigationViewPaneDisplayMode.LeftMinimal;
            }
        }

        private void OnSelectionChanged(
            Microsoft.UI.Xaml.Controls.NavigationView sender,
            Microsoft.UI.Xaml.Controls.NavigationViewSelectionChangedEventArgs args)
        {
            if (args.IsSettingsSelected)
            {
                ContentFrame.Navigate(typeof(SettingsPage), null, new EntranceNavigationTransitionInfo());
                return;
            }

            if (args.SelectedItem is not Microsoft.UI.Xaml.Controls.NavigationViewItem item
                || item.Tag is not string tag)
            {
                return;
            }

            var target = tag switch
            {
                "sessions" => typeof(SessionsPage),
                "workspaces" => typeof(WorkspacesPage),
                "tools" => typeof(ToolsPage),
                _ => typeof(SessionsPage),
            };

            if (ContentFrame.CurrentSourcePageType != target)
            {
                ContentFrame.Navigate(target, null, new EntranceNavigationTransitionInfo());
            }
        }

        /// <summary>搜索：把关键词传给会话页做筛选。</summary>
        private void OnSearchTextChanged(AutoSuggestBox sender, AutoSuggestBoxTextChangedEventArgs args)
        {
            if (args.Reason != AutoSuggestionBoxTextChangeReason.UserInput)
            {
                return;
            }

            if (ContentFrame.Content is SessionsPage page)
            {
                page.ApplyFilter(sender.Text);
            }
        }
    }
}
