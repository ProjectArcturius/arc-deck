using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.Linq;
using Windows.ApplicationModel.DataTransfer;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Media.Animation;
using Windows.UI.Xaml.Navigation;

namespace ArcDeck
{
    /// <summary>会话列表页。</summary>
    public sealed partial class SessionsPage : Page
    {
        private readonly ObservableCollection<Session> _view = new ObservableCollection<Session>();
        private string _keyword = string.Empty;

        public SessionsPage()
        {
            InitializeComponent();
            SessionList.ItemsSource = _view;
            ApplyFilter(string.Empty);
        }

        protected override void OnNavigatedTo(NavigationEventArgs e)
        {
            base.OnNavigatedTo(e);

            // 从对话页返回时重取一次 —— 那边发过消息，Title/Preview/时间都可能变了。
            // 用 x:Bind 的 OneWay 也做不到，因为 Session 的属性没有变更通知；
            // 重建视图集合是最直接且不会漏的方式。
            Refresh();
        }

        /// <summary>
        /// 按关键词筛选。由外壳的搜索框调用。
        ///
        /// 用「重建视图集合」而不是 CollectionViewSource —— 后者在这个规模下
        /// 是过度设计，直接过滤一遍更直白。
        /// </summary>
        public void ApplyFilter(string keyword)
        {
            _keyword = keyword ?? string.Empty;
            Refresh();
        }

        /// <summary>按当前筛选条件重建列表，并同步命令栏与空状态。</summary>
        private void Refresh()
        {
            // 先按「置顶 → 最后活动」排序数据源本身，视图只需按序添加。
            Store.Current.Resort();

            _view.Clear();

            var matched = string.IsNullOrWhiteSpace(_keyword)
                ? Store.Current.Sessions
                : Store.Current.Sessions.Where(Matches);

            foreach (var s in matched)
            {
                _view.Add(s);
            }

            var total = Store.Current.Sessions.Count;
            CountText.Text = string.IsNullOrWhiteSpace(_keyword)
                ? $"共 {total} 个会话"
                : $"匹配 {_view.Count} / {total} 个会话";

            var isEmpty = _view.Count == 0;
            EmptyState.Visibility = isEmpty ? Visibility.Visible : Visibility.Collapsed;
            EmptyHintText.Text = string.IsNullOrWhiteSpace(_keyword)
                ? "用上面的「新会话」开始一次对话。"
                : $"没有会话匹配「{_keyword}」。";

            UpdateCommandState();
        }

        private bool Matches(Session s)
        {
            return (s.Title?.Contains(_keyword, StringComparison.OrdinalIgnoreCase) ?? false)
                || (s.Preview?.Contains(_keyword, StringComparison.OrdinalIgnoreCase) ?? false)
                || (s.Workspace?.Contains(_keyword, StringComparison.OrdinalIgnoreCase) ?? false);
        }

        /// <summary>
        /// 菜单项的可用状态跟着选中项走。
        ///
        /// 不这么做的话，没选中任何会话时「删除」仍可点，点下去静默无反应 ——
        /// 用户会以为坏了。
        /// </summary>
        private void UpdateCommandState()
        {
            var hasSelection = SessionList.SelectedItems.Count > 0;
            var single = SessionList.SelectedItems.Count == 1;

            RenameMenuItem.IsEnabled = single;

            // 混选时置顶的文案取整体状态，避免出现「点了反而取消」的意外。
            var allPinned = hasSelection
                && SessionList.SelectedItems.Cast<Session>().All(s => s.IsPinned);

            PinMenuItem.Text = allPinned ? "取消置顶" : "置顶";
        }

        private void OnSelectionChanged(object sender, SelectionChangedEventArgs e)
            => UpdateCommandState();

        private IReadOnlyList<Session> Selected => SessionList.SelectedItems.Cast<Session>().ToList();

        private Session SingleSelected
            => SessionList.SelectedItems.Count == 1 ? (Session)SessionList.SelectedItems[0] : null;

        /// <summary>
        /// 右键菜单的目标行。
        ///
        /// 优先取右键记录；若用户是用键盘或命令栏操作，则回落到唯一选中项。
        /// 不能直接读 SelectedItems：右键不改变选中状态，先点 A 再右键 B 时
        /// 会作用到 A 上 —— 那是删错数据的错。
        /// </summary>
        private Session ContextTarget => SessionList.ContextItem as Session ?? SingleSelected;

        private void OnSessionClick(object sender, ItemClickEventArgs e)
        {
            if (e.ClickedItem is not Session session)
            {
                return;
            }

            // 多选状态下单击是在调整选择，不应该跳走 —— 否则没法批量操作。
            if (SessionList.SelectedItems.Count > 1)
            {
                return;
            }

            Open(session);
        }

        private void Open(Session session)
        {
            // DrillIn 是 UWP 给「进入层级」的专用转场，与同级切换的 Entrance 不同。
            // 直接用平台词汇，不自己发明动画。
            Frame.Navigate(typeof(ChatPage), session.Id, new DrillInNavigationTransitionInfo());
        }

        private void OnOpenClick(object sender, RoutedEventArgs e)
        {
            var target = SingleSelected ?? ContextTarget;
            if (target != null)
            {
                Open(target);
            }
        }

        private void OnNewSessionClick(object sender, RoutedEventArgs e)
        {
            var session = Store.Current.Create();
            Refresh();

            // 直接进新会话 —— 新建之后停在列表上还要再多点一次，是多余的一步。
            Open(session);
        }

        private void OnRefreshClick(object sender, RoutedEventArgs e) => Refresh();

        private void OnSelectAllClick(object sender, RoutedEventArgs e)
        {
            if (SessionList.SelectedItems.Count == _view.Count)
            {
                SessionList.SelectedItems.Clear();
            }
            else
            {
                SessionList.SelectAll();
            }
        }

        private async void OnRenameClick(object sender, RoutedEventArgs e)
        {
            var target = SingleSelected ?? ContextTarget;
            if (target == null)
            {
                return;
            }

            var box = new TextBox { Text = target.Title, SelectionStart = target.Title?.Length ?? 0 };
            var dialog = new ContentDialog
            {
                Title = "重命名会话",
                Content = box,
                PrimaryButtonText = "保存",
                CloseButtonText = "取消",
                DefaultButton = ContentDialogButton.Primary,
            };

            if (await dialog.ShowAsync() == ContentDialogResult.Primary)
            {
                Store.Current.Rename(target, box.Text);
                Refresh();
            }
        }

        private void OnPinClick(object sender, RoutedEventArgs e)
        {
            var targets = Selected.Count > 0
                ? Selected
                : (ContextTarget != null ? new List<Session> { ContextTarget } : new List<Session>());

            if (targets.Count == 0)
            {
                return;
            }

            // 整组统一到同一状态，而不是逐个取反 —— 后者在混选时结果不可预期。
            var shouldPin = targets.Any(s => !s.IsPinned);
            foreach (var s in targets)
            {
                s.IsPinned = shouldPin;
            }

            Refresh();
        }

        private void OnCopyPathClick(object sender, RoutedEventArgs e)
        {
            var target = SingleSelected ?? ContextTarget;
            if (target?.Workspace == null)
            {
                return;
            }

            var package = new DataPackage();
            package.SetText(target.Workspace);
            Clipboard.SetContent(package);
        }

        private async void OnDeleteClick(object sender, RoutedEventArgs e)
        {
            var targets = Selected.Count > 0
                ? Selected
                : (ContextTarget != null ? new List<Session> { ContextTarget } : new List<Session>());

            if (targets.Count == 0)
            {
                return;
            }

            // 删除不可撤销，所以确认一次。单个和批量给不同的措辞 ——
            // 「确定删除？」在批量时会让人数不清删了几个。
            var message = targets.Count == 1
                ? $"「{targets[0].Title}」将被删除，其中的消息一并移除。"
                : $"{targets.Count} 个会话将被删除，其中的消息一并移除。";

            var dialog = new ContentDialog
            {
                Title = "删除会话",
                Content = message,
                PrimaryButtonText = "删除",
                CloseButtonText = "取消",
                DefaultButton = ContentDialogButton.Close,
            };

            if (await dialog.ShowAsync() != ContentDialogResult.Primary)
            {
                return;
            }

            foreach (var s in targets)
            {
                Store.Current.Delete(s);
            }

            Refresh();
        }
    }
}
