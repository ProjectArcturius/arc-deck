namespace ArcDeck.Dsh.Harness;

/// <summary>
/// 一次 harness web 部署所需的路径。
///
/// 与 <see cref="DshRuntimeLayout"/> 同源但**不是同一个东西**，所以单独一个类型：
/// SDK 那条路要的是 <c>--profile sdk</c> 的 profile 目录，web 这条路要的是
/// **web profile**（DSH Desktop 装好的那一份）。两者都用同一个 DSH_HOME，
/// 但 profile 内容完全不同 —— 混用一个字段会让「起不来」变成一件要靠猜的事。
/// </summary>
public sealed record HarnessInstall(
    string NodeExecutable,
    string DshEntry,
    string HarnessHome,
    string WebProfileDirectory,
    string Source)
{
    public override string ToString() =>
        $"node={NodeExecutable}\n  dsh={DshEntry}\n  DSH_HOME={HarnessHome}\n  web profile={WebProfileDirectory}\n  来源={Source}";
}

public static class HarnessLocator
{
    /// <summary>
    /// 找一份能跑 <c>dsh web</c> 的安装。
    ///
    /// 判据比 SDK 那条路严格一点：**还要求 web profile 存在**。
    /// 只找到 dsh 入口但 profile 没装时，`dsh web` 会在启动期失败，
    /// 而那条失败信息（插件树装不起来）离「profile 没装」很远。
    /// </summary>
    public static HarnessInstall? Locate(string? harnessHome = null)
    {
        harnessHome ??= DshRuntimeLocator.DefaultHarnessHome();

        foreach (var root in CandidateRoots())
        {
            if (string.IsNullOrWhiteSpace(root) || !Directory.Exists(root)) continue;

            var appRoot = Path.Combine(root, "resources", "app");
            var entry = Path.Combine(appRoot, "node_modules", "@deepseek-ai", "dsh", "lib", "bin.js");
            if (!File.Exists(entry)) continue;

            var node = FindNode(appRoot);
            if (node is null) continue;

            var profile = Path.Combine(harnessHome, "profiles", "web");
            if (!Directory.Exists(profile)) continue;

            return new HarnessInstall(node, entry, harnessHome, profile, root);
        }

        return null;
    }

    private static IEnumerable<string> CandidateRoots()
    {
        var over = Environment.GetEnvironmentVariable("ARC_DECK_DSH_ROOT");
        if (!string.IsNullOrWhiteSpace(over)) yield return over!;

        yield return Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Programs", "DSH Desktop");
        yield return Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),
            "DSH Desktop");
        yield return Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "Programs", "dsh-desktop");
    }

    private static string? FindNode(string appRoot)
    {
        var local = Path.Combine(appRoot, "node_modules", "node", "bin",
            OperatingSystem.IsWindows() ? "node.exe" : "node");
        if (File.Exists(local)) return local;

        foreach (var dir in (Environment.GetEnvironmentVariable("PATH") ?? string.Empty)
                 .Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            var candidate = Path.Combine(dir, OperatingSystem.IsWindows() ? "node.exe" : "node");
            if (File.Exists(candidate)) return candidate;
        }

        return null;
    }
}
