using System.Runtime.InteropServices;

namespace ArcDeck.Dsh;

/// <summary>
/// 一次运行所需的全部路径 —— 「内核在哪」这个问题的**唯一**答案。
///
/// 为什么把它单独做成一个类型而不是几个字符串参数：这三个路径必须互相一致
/// （node 要能跑这个 dsh entry，profile 要落在同一个 DSH_HOME 下），
/// 分开传参迟早会出现「用 A 的 node 跑 B 的入口、读 C 的凭据」这类错配，
/// 而那种错配的表现是「模型不可用」之类的间接症状，很难回溯到路径。
/// </summary>
public sealed record DshRuntimeLayout(
    string NodeExecutable,
    string DshEntry,
    string HarnessHome,
    string ProfileDirectory,
    string Source)
{
    public override string ToString() =>
        $"node={NodeExecutable}\n  dsh={DshEntry}\n  DSH_HOME={HarnessHome}\n  profile={ProfileDirectory}\n  来源={Source}";
}

public static class DshRuntimeLocator
{
    /// <summary>
    /// 查找一个可用的 dsh 安装。
    ///
    /// 搜索顺序是有理由的，不要随意调整：
    ///   1. <c>ARC_DECK_DSH_ROOT</c> —— 显式覆盖，排查时把变量一设就能验证「换一份
    ///      安装是不是就好了」，不需要改代码
    ///   2. Program Files / LocalAppData 上的 DSH Desktop 自带副本 —— 本机现状；
    ///      它自带 node.exe，所以不依赖 PATH
    ///   3. 用户级 <c>~/.dsh</c> 里由 pnpm 装出来的 profile（如果将来有）
    ///
    /// **找不到就返回 null，不抛**：调用方要把「没装内核」显示成一个可读的状态
    /// （界面上一行字），而不是让应用在启动时崩掉。装没装内核不是本应用的
    /// 先决条件，只是「能不能发消息」的前提。
    /// </summary>
    public static DshRuntimeLayout? Locate(string? harnessHome = null)
    {
        harnessHome ??= DefaultHarnessHome();

        foreach (var root in CandidateRoots())
        {
            if (string.IsNullOrWhiteSpace(root) || !Directory.Exists(root)) continue;

            var appRoot = Path.Combine(root, "resources", "app");
            var entry = Path.Combine(appRoot, "node_modules", "@deepseek-ai", "dsh", "lib", "bin.js");
            if (!File.Exists(entry)) continue;

            var node = FindNode(root, appRoot);
            if (node is null) continue;

            return new DshRuntimeLayout(
                NodeExecutable: node,
                DshEntry: entry,
                HarnessHome: harnessHome,
                ProfileDirectory: Path.Combine(harnessHome, "profiles", "sdk"),
                Source: root);
        }

        return null;
    }

    private static IEnumerable<string> CandidateRoots()
    {
        // 1 · 显式覆盖
        var over = Environment.GetEnvironmentVariable("ARC_DECK_DSH_ROOT");
        if (!string.IsNullOrWhiteSpace(over)) yield return over!;

        // 2 · DSH Desktop 的默认安装位置（用户级 / 全机级）
        yield return Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Programs", "DSH Desktop");
        yield return Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),
            "DSH Desktop");

        // 3 · 官方支持的另一个落点（VS Code 风格的 per-user 安装）
        yield return Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Programs", "dsh-desktop");
    }

    /// <summary>
    /// 找 node。**优先用 DSH Desktop 自带的那个**，而不是 PATH 上的。
    ///
    /// 理由：自带的是 24.9.0 且与那份 dsh 一起发布的；PATH 上可能是任意版本，
    /// 而内核在启动期会 <c>--expose-internals</c>，版本差异会以「起不来」的形式出现，
    /// 排查成本远高于这里多写两行。
    /// </summary>
    private static string? FindNode(string installRoot, string appRoot)
    {
        var local = Path.Combine(appRoot, "node_modules", "node", "bin",
            OperatingSystem.IsWindows() ? "node.exe" : "node");
        if (File.Exists(local)) return local;

        var legacy = Path.Combine(appRoot, "node_modules", "node", "bin", "node.exe");
        if (File.Exists(legacy)) return legacy;

        // PATH 兜底 —— 只在自带副本缺失时走到这里。
        var pathVar = Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
        foreach (var dir in pathVar.Split(Path.PathSeparator))
        {
            if (string.IsNullOrWhiteSpace(dir)) continue;
            var candidate = Path.Combine(dir, OperatingSystem.IsWindows() ? "node.exe" : "node");
            if (File.Exists(candidate)) return candidate;
        }

        return null;
    }

    /// <summary>
    /// 内核的 home。默认跟随 DSH Desktop 的那一份。
    ///
    /// 用同一份 home 是刻意的：凭据（<c>.credentials.yaml</c>）、settings 与
    /// 网络代理配置都在那里。换一个空 home 会得到「适配器起来了但请求失败」——
    /// 而那条错误信息看起来像网络问题。
    /// </summary>
    public static string DefaultHarnessHome()
    {
        var env = Environment.GetEnvironmentVariable("DSH_HOME");
        if (!string.IsNullOrWhiteSpace(env)) return env!;

        var roaming = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
        var desktop = Path.Combine(roaming, "dsh-desktop", "harness");
        if (Directory.Exists(desktop)) return desktop;

        var userProfile = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        return Path.Combine(userProfile, ".dsh");
    }
}
