using System.Collections.Generic;

namespace ArcDeck
{
    /// <summary>
    /// 一个可供模型调用的工具。
    ///
    /// 字段对应 DSH 协议里工具描述的形状：名称、说明、参数、风险级别。
    /// 目前由 <see cref="Store"/> 提供示例数据 —— 接上协议后这张表来自上游，
    /// 界面不需要改。
    /// </summary>
    public sealed class ToolInfo
    {
        public string Name { get; set; }

        /// <summary>一句话说明「这个工具做什么」，用来在列表里区分同名工具。</summary>
        public string Summary { get; set; }

        /// <summary>参数签名，例如 "path: string, offset?: int"。</summary>
        public string Signature { get; set; }

        /// <summary>来源：内置 / 项目自定义 / MCP。</summary>
        public string Origin { get; set; }

        /// <summary>
        /// 是否需要人工确认才能执行。
        ///
        /// 真实客户端里这是最重要的一个字段 —— 它决定哪些调用会打断用户。
        /// 标出来比让用户逐个记要可靠。
        /// </summary>
        public bool RequiresApproval { get; set; }

        /// <summary>说明段落，详情区显示。</summary>
        public IList<string> Details { get; set; } = new List<string>();

        /// <summary>列表里显示的徽标文字。空字符串表示不显示徽标。</summary>
        public string Badge => RequiresApproval ? "需确认" : string.Empty;

        /// <summary>
        /// 列表行的一行摘要：来源 · 参数。
        /// 拼在这里而不是 XAML 里，避免在模板里堆字符串拼接。
        /// </summary>
        public string Subtitle => string.IsNullOrEmpty(Signature)
            ? Origin
            : $"{Origin} · {Signature}";
    }
}
