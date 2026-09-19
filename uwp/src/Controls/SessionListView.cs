using System;
using System.Collections.Generic;
using Windows.Foundation;
using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;
using Windows.UI.Xaml.Input;

namespace ArcDeck
{
    /// <summary>
    /// 会话列表。存在的唯一理由是**知道右键点的是哪一行**。
    ///
    /// ── 为什么必须自己派生一个 ListView ──────────────────────────────
    ///
    /// UWP 不提供「被右键的那一项」。`ContextFlyout` 挂在 ListView 上时，
    /// 菜单项的 Click 处理器只能去读 `SelectedItems`，而**右键不改变选中状态**
    /// —— 结果就是先点 A 再右键 B，菜单里的「重命名」「删除」作用在 A 上。
    /// 这是很隐蔽的错：单次使用看不出来，一旦按那个顺序操作就会改错数据。
    ///
    /// WPF 用 `Style` 的 `EventSetter` 解决，**UWP 不支持 EventSetter**。
    ///
    /// ── 走过的两条错路 ─────────────────────────────────────────────
    ///
    /// 1. `GetContainerForItemOverride` 里给容器
    ///    `AddHandler(RightTappedEvent, ..., handledEventsToo: true)`。
    ///    **实测不生效**：右键后 `ContextItem` 仍是 null，页面上的诊断输出为
    ///    `sel=0 ctx=null tg=0`。原因是 `RightTapped` 是手势识别事件，
    ///    用合成鼠标消息（`mouse_event`）驱动时不一定会走完手势识别管道。
    ///
    /// 2. 读 `SelectedItems` —— 正是上面说的语义错误。
    ///
    /// ── 现在用的办法 ───────────────────────────────────────────────
    ///
    /// 在容器上记 `PointerPressed` 的坐标（右键按下一定会走这里，合成消息也走），
    /// 然后在菜单 `Opening` 时用那个坐标反查命中的行。不依赖手势识别，
    /// 语义也正确：菜单永远作用于它弹出的那一行。
    ///
    /// 容器矩形用 `TransformToVisual` 换算成相对本控件的坐标，不读屏幕坐标 ——
    /// 免得多显示器与 DPI 缩放把换算搞错。
    /// </summary>
    public sealed class SessionListView : ListView
    {
        /// <summary>被右键的那一行。页面在菜单 Click 里读它。</summary>
        public object ContextItem { get; private set; }

        /// <summary>
        /// 行右键菜单。由页面赋值（和它的处理器绑定在一起）。
        ///
        /// 之所以让页面来给而不是本控件内建：菜单项要调页面上的处理器，
        /// 控件不该知道页面的方法名。这里只负责**把它挂到每个容器上**，
        /// 并在弹出时记录目标行。
        /// </summary>
        public MenuFlyout RowMenu { get; set; }

        /// <summary>最后一次按下的位置（相对本控件）。菜单弹出时用它反查行。</summary>
        Point? _lastPointerPosition;

        protected override DependencyObject GetContainerForItemOverride()
        {
            var item = (ListViewItem)base.GetContainerForItemOverride();

            // 记坐标。PointerPressed 是原始指针事件，合成鼠标消息也能触发；
            // 手势类的 RightTapped 则不一定（见类型注释里的错路 1）。
            item.PointerPressed += OnContainerPointerPressed;

            item.ContextFlyout = RowMenu;

            if (RowMenu != null)
            {
                RowMenu.Opening += (_, __) => CaptureContextItem();
            }

            return item;
        }

        void OnContainerPointerPressed(object sender, PointerRoutedEventArgs e)
        {
            var point = e.GetCurrentPoint(this);
            _lastPointerPosition = point.Position;
        }

        /// <summary>
        /// 用最后记录的指针位置找出所在的行，记进 <see cref="ContextItem"/>。
        ///
        /// 取不到位置时（例如键盘唤出菜单）保持原值不动 —— 让页面回落到
        /// 「唯一选中项」。
        /// </summary>
        void CaptureContextItem()
        {
            if (_lastPointerPosition == null)
            {
                return;
            }

            var pos = _lastPointerPosition.Value;

            for (var i = 0; i < Items.Count; i++)
            {
                if (ContainerFromIndex(i) is not ListViewItem container)
                {
                    continue;
                }

                if (BoundsInThis(container).Contains(pos))
                {
                    ContextItem = container.Content;
                    return;
                }
            }
        }

        /// <summary>容器相对本控件的矩形。用相对坐标而不是屏幕坐标。</summary>
        Rect BoundsInThis(ListViewItem container)
        {
            try
            {
                var transform = container.TransformToVisual(this);
                var origin = transform.TransformPoint(new Point(0, 0));
                return new Rect(origin, container.RenderSize);
            }
            catch (ArgumentException)
            {
                // 元素还没进视觉树时 TransformToVisual 会抛。跳过这一行。
                return Rect.Empty;
            }
        }
    }
}
