using Windows.UI.Xaml;
using Windows.UI.Xaml.Controls;

namespace ArcDeck
{
    /// <summary>
    /// 按消息归属分派模板。
    ///
    /// UWP 的标准做法：<see cref="DataTemplateSelector"/> 比在单一模板里堆
    /// 绑定转换器清楚，选择的模板名在调试器里也直接可见。
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

    /// <summary>
    /// 按内容段类型分派模板（段落 / 代码块 / 工具调用）。
    ///
    /// 三条分支各自独立排版，比在一个模板里叠三层 Visibility 清楚得多。
    /// </summary>
    public sealed class BlockTemplateSelector : DataTemplateSelector
    {
        public DataTemplate Para { get; set; }
        public DataTemplate Code { get; set; }
        public DataTemplate Tool { get; set; }

        protected override DataTemplate SelectTemplateCore(object item)
        {
            var kind = item is ChatBlock b ? b.Kind : BlockKind.Paragraph;
            return kind switch
            {
                BlockKind.Code => Code,
                BlockKind.Tool => Tool,
                _ => Para,
            };
        }

        protected override DataTemplate SelectTemplateCore(object item, DependencyObject container)
            => SelectTemplateCore(item);
    }
}
