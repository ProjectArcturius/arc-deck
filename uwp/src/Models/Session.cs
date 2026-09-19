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
        public string UpdatedAt { get; set; }
        public int TurnCount { get; set; }

        /// <summary>该会话的消息。</summary>
        public ObservableCollection<ChatMessage> Messages { get; } = new ObservableCollection<ChatMessage>();

        /// <summary>列表里显示的副标题。</summary>
        public string Subtitle => $"{Workspace} · {TurnCount} 轮";

        /// <summary>
        /// 头像里显示的首字。
        ///
        /// 单独给一个属性而不是在 XAML 里写 <c>Title[0]</c> —— x:Bind 不支持
        /// 字符串索引器（报「Unexpected array indexer」）。
        /// </summary>
        public string Initial => string.IsNullOrEmpty(Title) ? "?" : Title.Substring(0, 1);
    }
}
