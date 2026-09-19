using System;
using System.Collections.ObjectModel;
using Windows.UI.Xaml.Media;

namespace ArcDeck
{
    /// <summary>对话中的一条消息。</summary>
    public sealed class ChatMessage
    {
        /// <summary>角色显示名（"你" / "助手" / 工具名）。</summary>
        public string Author { get; set; }

        /// <summary>正文。</summary>
        public string Text { get; set; }

        /// <summary>
        /// 是否为自己发的。决定气泡对齐方向 —— 这是对话 UI 的通用约定，
        /// UWP 的 ListView 用 DataTemplateSelector 或绑定转换器表达。
        /// </summary>
        public bool IsMine { get; set; }

        /// <summary>
        /// 工具调用卡片（可选）。非空时在消息下方渲染一张卡片。
        /// </summary>
        public string ToolName { get; set; }

        /// <summary>工具调用的参数或命令。</summary>
        public string ToolDetail { get; set; }

        /// <summary>工具输出。</summary>
        public string ToolOutput { get; set; }

        public bool HasTool => !string.IsNullOrEmpty(ToolName);
    }
}
