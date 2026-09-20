namespace ArcDeck.Dsh;

/// <summary>
/// 与本客户端对话的上游版本。
///
/// **为什么把版本写成常量**：上游协议**没有版本协商** —— 握手只回一个
/// <c>serverInfo.version = "0.0.1"</c>，而且客户端不校验（上游文档原文）。
/// 也就是说「对侧到底是什么」在协议里问不出来。既然问不出来，就只能在
/// 客户端这一侧钉死一个已知可用的组合，升级时改这里，改动是刻意的。
///
/// 上游在 0.1.2 → 0.1.6 之间重构过两次协议形状，所以这个常量不是装饰。
/// </summary>
public static class DshVersion
{
    /// <summary>已验证过的 dsh 内核版本（DSH Desktop 0.8.2 自带的那份）。</summary>
    public const string PinnedRuntimeVersion = "0.1.2-rc.1";

    public const string ProtocolServerName = "deepseek-harness-sdk-runtime";

    /// <summary>provider —— 路由名，与模型名**不是**同一个东西。</summary>
    public const string DefaultProvider = "deepseek-official";

    /// <summary>
    /// model。
    ///
    /// **踩过一次**：最初这里填的是 <c>"deepseek-official"</c>（把 provider
    /// 当模型名用了），结果是内核接受了握手、turn 也起了，却只在
    /// <c>assistant/chunk</c> 的 <c>finish</c> 块里回一句
    /// 「The supported API model names are deepseek-flash, deepseek-v4-pro,
    /// but you passed deepseek-official.」—— 界面上表现为「发出去以后什么都没有」。
    ///
    /// 也就是说：**provider 与 model 校验发生在轮次内部，不在 initialize 里**，
    /// 所以这个错误不会在握手时暴露。改这一行之前先确认选中的 provider 支持它。
    /// </summary>
    public const string DefaultModel = "deepseek-flash";

    /// <summary>
    /// initialize 的等待上限。
    ///
    /// 为什么给这么长：内核会等整棵插件树加载完（含 MCP 工具发现）才回这一帧，
    /// 上游明确要求客户端在此之后才能发 prompt。慢机器上十几秒是正常的，
    /// 给 3 秒会让它在慢机器上随机失败 —— 而那种失败看起来像「协议不对」。
    /// </summary>
    public static readonly TimeSpan InitializeTimeout = TimeSpan.FromMinutes(2);

    /// <summary>session/prompt 的回执很快（只是入队），超时短一点能让卡死更早暴露。</summary>
    public static readonly TimeSpan PromptAcceptTimeout = TimeSpan.FromSeconds(30);
}
