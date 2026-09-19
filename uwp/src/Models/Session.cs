using System;
using System.Collections.ObjectModel;
using System.Linq;

namespace ArcDeck
{
    /// <summary>一个会话。</summary>
    public sealed class Session
    {
        public string Id { get; set; }
        public string Title { get; set; }
        public string Preview { get; set; }
        public string Workspace { get; set; }

        /// <summary>最后活动时间。真实时间戳，不是预格式化的字符串。</summary>
        public DateTimeOffset UpdatedAt { get; set; } = DateTimeOffset.Now;

        /// <summary>是否置顶。</summary>
        public bool IsPinned { get; set; }

        public int TurnCount { get; set; }

        /// <summary>该会话的消息。</summary>
        public ObservableCollection<ChatMessage> Messages { get; } = new ObservableCollection<ChatMessage>();

        /// <summary>列表里显示的副标题。</summary>
        public string Subtitle => $"{Workspace} · {TurnCount} 轮";

        /// <summary>
        /// 相对时间（"刚刚" / "12 分钟前" / "昨天"）。
        ///
        /// 为什么用计算属性而不是存字符串：原先 <c>UpdatedAt</c> 就是个字面量
        /// ("刚刚"、"12 分钟前")，那是死的 —— 应用开着不动，时间也不会变。
        /// 现在按真实时间戳算，界面刷新时重新取一次即可。
        /// </summary>
        public string UpdatedAtText
        {
            get
            {
                var delta = DateTimeOffset.Now - UpdatedAt;

                if (delta.TotalSeconds < 60)
                {
                    return "刚刚";
                }

                if (delta.TotalMinutes < 60)
                {
                    return $"{(int)delta.TotalMinutes} 分钟前";
                }

                // 同一天内按小时算，避免"23 小时前"其实是今天早上这种别扭说法。
                if (UpdatedAt.Date == DateTimeOffset.Now.Date)
                {
                    return $"{(int)delta.TotalHours} 小时前";
                }

                if (delta.TotalDays < 2)
                {
                    return "昨天";
                }

                if (delta.TotalDays < 30)
                {
                    return $"{(int)delta.TotalDays} 天前";
                }

                // 超过一个月就给绝对日期 —— 相对时间在这个尺度上不再帮助判断。
                return UpdatedAt.ToString("yyyy/MM/dd");
            }
        }

        /// <summary>
        /// 头像里显示的首字。
        ///
        /// 单独给一个属性而不是在 XAML 里写 <c>Title[0]</c> —— x:Bind 不支持
        /// 字符串索引器（报「Unexpected array indexer」）。
        /// </summary>
        public string Initial => string.IsNullOrEmpty(Title) ? "?" : Title.Substring(0, 1);

        /// <summary>
        /// 从消息重算 Preview 与轮数。
        ///
        /// 发完消息要调用 —— 否则列表里显示的仍是几轮之前的摘要，
        /// 那是"看起来能用但数据是假的"最典型的一种。
        /// </summary>
        public void Touch()
        {
            UpdatedAt = DateTimeOffset.Now;
            TurnCount = Messages.Count(m => m.IsMine);

            var last = Messages.LastOrDefault();
            if (last == null)
            {
                return;
            }

            var firstBlock = last.Blocks.FirstOrDefault();
            if (firstBlock != null && !string.IsNullOrWhiteSpace(firstBlock.Text))
            {
                Preview = firstBlock.Text;
            }
        }
    }
}
