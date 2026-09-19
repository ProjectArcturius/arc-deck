using System.Collections.ObjectModel;

namespace ArcDeck
{
    /// <summary>内容段类型。</summary>
    public enum BlockKind
    {
        /// <summary>普通段落。</summary>
        Paragraph,

        /// <summary>代码块 —— 等宽、可选语言标签。</summary>
        Code,

        /// <summary>工具调用 —— 带名称与输出。</summary>
        Tool,
    }

    /// <summary>对话里的一段内容。</summary>
    public sealed class ChatBlock
    {
        public BlockKind Kind { get; set; }

        /// <summary>正文。代码块与工具输出的原文也放这里。</summary>
        public string Text { get; set; }

        /// <summary>代码块的语言标签（可选）。</summary>
        public string Language { get; set; }

        /// <summary>工具名（仅工具块）。</summary>
        public string ToolName { get; set; }

        /// <summary>工具是否成功（决定标题图标）。</summary>
        public bool ToolOk { get; set; } = true;

        /// <summary>代码块是否带语言标签。</summary>
        public bool HasLanguage => Kind == BlockKind.Code && !string.IsNullOrEmpty(Language);

        /// <summary>工具块显示名。</summary>
        public string ToolHeader => ToolOk ? ToolName : ToolName + "（失败）";
    }

    /// <summary>
    /// 对话中的一条消息。
    ///
    /// 内容用 <see cref="ChatBlock"/> 列表而不是单个字符串 —— 真实对话里段落、
    /// 代码、工具调用是混排的，用单一字符串会在模板里堆一堆可见性判断。
    /// </summary>
    public sealed class ChatMessage
    {
        /// <summary>角色显示名（"你" / "助手"）。保留给无障碍与将来复用。</summary>
        public string Author { get; set; }

        /// <summary>是否为自己发的。决定靠左还是靠右。</summary>
        public bool IsMine { get; set; }

        /// <summary>
        /// 元信息一行：时间 · 耗时 · 轮次 · 模型。
        ///
        /// 为什么放进模型而不是在模板里拼：真实客户端里这些字段来源不同
        /// （本地时钟 / 上游 usage / 会话状态），在模型层合成一处，界面只负责显示。
        /// </summary>
        public string Meta { get; set; }

        /// <summary>内容的段落序列。</summary>
        public ObservableCollection<ChatBlock> Blocks { get; } = new ObservableCollection<ChatBlock>();

        public static ChatMessage Mine(params string[] paragraphs)
        {
            var m = new ChatMessage { Author = "你", IsMine = true, Meta = "你" };
            foreach (var p in paragraphs)
            {
                m.Blocks.Add(new ChatBlock { Kind = BlockKind.Paragraph, Text = p });
            }
            return m;
        }

        public static ChatMessage Theirs(params string[] paragraphs)
        {
            var m = new ChatMessage { Author = "助手", IsMine = false, Meta = "助手" };
            foreach (var p in paragraphs)
            {
                m.Blocks.Add(new ChatBlock { Kind = BlockKind.Paragraph, Text = p });
            }
            return m;
        }

        /// <summary>设置元信息行。</summary>
        public ChatMessage At(string meta)
        {
            Meta = meta;
            return this;
        }

        public ChatMessage WithCode(string language, string code)
        {
            Blocks.Add(new ChatBlock { Kind = BlockKind.Code, Language = language, Text = code });
            return this;
        }

        public ChatMessage WithTool(string name, string output, bool ok = true)
        {
            Blocks.Add(new ChatBlock
            {
                Kind = BlockKind.Tool,
                ToolName = name,
                Text = output,
                ToolOk = ok,
            });
            return this;
        }
    }
}
