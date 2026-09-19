using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Input;

namespace ArcDeck
{
    /// <summary>
    /// 会话列表。存在的唯一理由是**右键目标行**。
    ///
    /// 为什么必须自己派生一个 ListView：UWP 里拿不到「被右键的那一行」。
    /// `ContextFlyout` 挂在 ListView 上时，菜单项的 Click 处理器不知道目标是谁，
    /// 只能去读 SelectedItems —— 而右键不改变选中状态，结果就是菜单里的
    /// 「重命名」「删除」作用在上一次选中的会话上。这是很隐蔽的错：
    /// 单次使用看不出来，一旦先点 A 再右键 B 就会删错。
    ///
    /// WPF 用 `Style` 的 `EventSetter` 解决这个问题，**UWP 不支持 EventSetter**，
    /// 所以只能在容器生成时挂事件。
    ///
    /// 用 `AddHandler(..., handledEventsToo: true)` 而不是 `+=`：ListViewItem
    /// 自己会处理 RightTapped 来弹 ContextFlyout，等事件冒泡到页面时已经标了
    /// Handled，普通订阅收不到。
    /// </summary>
    public sealed class SessionListView : ListView
    {
        /// <summary>被右键的那一行。页面在菜单 Click 里读它。</summary>
        public object ContextItem { get; private set; }

        protected override DependencyObject GetContainerForItemOverride()
        {
            var item = (ListViewItem)base.GetContainerForItemOverride();

            item.AddHandler(
                RightTappedEvent,
                new RightTappedEventHandler(OnContainerRightTapped),
                handledEventsToo: true);

            return item;
        }

        private void OnContainerRightTapped(object sender, RightTappedRoutedEventArgs e)
        {
            ContextItem = (sender as FrameworkElement)?.DataContext;
        }
    }
}
