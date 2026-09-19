using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;

namespace ArcDeck
{
    /// <summary>
    /// 按消息归属分派模板。
    ///
    /// UWP 的标准做法：<see cref="DataTemplateSelector"/> 比在单一模板里堆
    /// 一堆绑定转换器清楚得多，而且选择的模板名在调试器里可直接看见。
    /// </summary>
    public sealed class MessageTemplateSelector : DataTemplateSelector
    {
        /// <summary>自己的消息（靠右）。</summary>
        public DataTemplate Mine { get; set; }

        /// <summary>对方的消息（靠左）。</summary>
        public DataTemplate Theirs { get; set; }

        protected override DataTemplate SelectTemplateCore(object item)
            => item is ChatMessage m && m.IsMine ? Mine : Theirs;

        protected override DataTemplate SelectTemplateCore(object item, DependencyObject container)
            => SelectTemplateCore(item);
    }
}
