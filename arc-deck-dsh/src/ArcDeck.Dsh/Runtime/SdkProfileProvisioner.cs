namespace ArcDeck.Dsh.Runtime;

/// <summary>
/// 准备 sdk profile —— 让 dsh 认得住 <c>--profile sdk</c>。
///
/// 为什么客户端要自己写 profile，而不是复用 DSH Desktop 的 <c>web</c> profile：
/// 那个 profile 的组合是 web 应用（WebSocket 服务 + 前端），它的 stdio 上不说话。
/// SDK 的 stdio 协议由 <c>@deepseek-ai/dsh-sdk-app</c> 这个 bundle 提供。
///
/// 目录结构照 DSH Desktop 的 web profile 抄（那是本机唯一一份已知可用的实例）：
///
///     <c>&lt;DSH_HOME&gt;/profiles/sdk/cordis.yml</c>          空入口表，组合由 bundle 提供
///     <c>&lt;DSH_HOME&gt;/profiles/sdk/package.json</c>         声明 bundle 清单
///     <c>&lt;DSH_HOME&gt;/profiles/sdk/cordis.patch.yml</c>     用户的 patch 层
///
/// **只在缺失时写**：这三个文件里有用户在 patch 层做过的调整，覆盖写会把它冲掉；
/// 这也是「profile 已经存在就不再动它」的原因 —— 同一条原则在 DSH Desktop 里
/// 表现为 <c>.install-complete</c> 标记，这里用「文件在不在」代替，够用且少一个状态。
/// </summary>
internal static class SdkProfileProvisioner
{
    internal const string BundlesJson = """
        {
          "name": "dsh-profile-sdk",
          "private": true,
          "dependencies": {},
          "dsh": {
            "profile": {
              "bundles": [
                "@deepseek-ai/dsh-base",
                "@deepseek-ai/dsh-sdk-app"
              ],
              "patchReload": "startup"
            }
          }
        }
        """;

    private const string CordisYml = """
        # dsh profile root —— 空入口表，组合由 package.json 里的 bundle 提供。
        # 与 DSH Desktop 的 web profile 同构；改组合请改 cordis.patch.yml，不要改这里。
        []
        """;

    private const string PatchYml = """
        # 本 profile 的 patch 层，在每个 bundle 层之后应用（loader patch 数组）。
        []
        """;

    internal static void Ensure(string profileDirectory)
    {
        Directory.CreateDirectory(profileDirectory);
        WriteIfMissing(Path.Combine(profileDirectory, "cordis.yml"), CordisYml);
        WriteIfMissing(Path.Combine(profileDirectory, "package.json"), BundlesJson);
        WriteIfMissing(Path.Combine(profileDirectory, "cordis.patch.yml"), PatchYml);
    }

    private static void WriteIfMissing(string path, string content)
    {
        if (File.Exists(path)) return;
        File.WriteAllText(path, content, new System.Text.UTF8Encoding(false));
    }
}
