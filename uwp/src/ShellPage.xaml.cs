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

            // 主题必须在 InitializeComponent() **之前**设好。
            //
            // 踩过两次坑，都是顺序或作用对象不对：
            //
            //  1. 起初把主题应用到 `Window.Content`（那个 Frame）—— 只有内容区
            //     变亮，导航窗格仍是暗的。NavigationView 是 Frame 的**父级**，
            //     而 RequestedTheme 在视觉树上只向下传播。
            //  2. 改到 Nav 上、但在 InitializeComponent() 之后设 —— 也一样。
            //
            // 所以顺序是：先给一个尚未加载的元素设 RequestedTheme，再解析 XAML。
            // 本页在构造函数里还没进视觉树，正好满足这个条件。
            Settings.ApplyTheme(this);

            InitializeComponent();
            Nav.SelectedItem = MenuItems[0];
        }

        /// <summary>
        /// 重新应用主题。设置页改完主题后调用，让内容区立即变色并落到设置里。
        ///
        /// **已知局限（实测，未修）**：NavigationView 的**窗格背景**不跟着变。
        /// 它是平台在加载期解析的 AcrylicBrush（NavigationViewDefaultPaneBackground，
        /// 由 SDK 的 themeresources.xaml 用 StaticResource 指向），事后改
        /// RequestedTheme 不会让它重新解析。实测浅色主题下内容区 #808080、
        /// 窗格仍是 #000000。
        ///
        /// 不修的理由：唯一的修法是覆盖那个平台画刷，而 U1 禁止往平台外观上塞
        /// 自定义配色 —— 那会把「跟系统走」这条路走死，代价大于收益。
        /// 记在 AGENTS.md U9。
        /// </summary>
        public void ReloadTheme() => Settings.ApplyTheme(this);

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
