using System;
using System.Collections.ObjectModel;
using System.Linq;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Media.Animation;
using Windows.UI.Xaml.Navigation;

namespace ArcDeck
{
    /// <summary>会话列表页。</summary>
    public sealed partial class SessionsPage : Page
    {
        private readonly ObservableCollection<Session> _view = new ObservableCollection<Session>();

        public SessionsPage()
        {
            InitializeComponent();
            SessionList.ItemsSource = _view;
            ApplyFilter(string.Empty);
        }

        /// <summary>
        /// 按关键词筛选。由外壳的搜索框调用。
        ///
        /// 用「重建视图集合」而不是 CollectionViewSource —— 后者在这个规模下
        /// 是过度设计，直接过滤一遍更直白。
        /// </summary>
        public void ApplyFilter(string keyword)
        {
            _view.Clear();

            var matched = string.IsNullOrWhiteSpace(keyword)
                ? Store.Current.Sessions
                : Store.Current.Sessions.Where(s =>
                    (s.Title?.Contains(keyword, StringComparison.OrdinalIgnoreCase) ?? false)
                    || (s.Preview?.Contains(keyword, StringComparison.OrdinalIgnoreCase) ?? false)
                    || (s.Workspace?.Contains(keyword, StringComparison.OrdinalIgnoreCase) ?? false));

            foreach (var s in matched)
            {
                _view.Add(s);
            }

            CountText.Text = string.IsNullOrWhiteSpace(keyword)
                ? $"共 {_view.Count} 个会话"
                : $"匹配 {_view.Count} / {Store.Current.Sessions.Count} 个会话";
        }

        private void OnSessionClick(object sender, ItemClickEventArgs e)
        {
            if (e.ClickedItem is not Session session)
            {
                return;
            }

            // DrillIn 是 UWP 给「进入层级」的专用转场，与同级切换的 Entrance 不同。
            // 直接用平台词汇，不自己发明动画。
            Frame.Navigate(typeof(ChatPage), session.Id, new DrillInNavigationTransitionInfo());
        }
    }
}
