using System.Diagnostics;
using System.Text;
using System.Text.Json;

namespace ArcDeck.Host;

/// <summary>
/// Every entry point appends here. The process is launched by Windows, has no console, and its
/// failures are otherwise invisible — a silent no-op would be indistinguishable from "the UWP
/// side never launched it".
/// </summary>
internal static class HostLog
{
    public static readonly string Path =
        System.IO.Path.Combine(System.IO.Path.GetTempPath(), "arcdeck-host.log");

    public static void Write(string line)
    {
        try
        {
            File.AppendAllText(Path, $"{DateTime.Now:HH:mm:ss.fff} [{Environment.ProcessId}] {line}{Environment.NewLine}");
        }
        catch
        {
            // Logging must never take the process down with it.
        }
    }
}

/// <summary>One request/reply frame. JSON lives in a single ValueSet string so the two sides
/// never have to agree on ValueSet type tags.</summary>
internal sealed class Request
{
    public string? Id { get; set; }
    public string? Op { get; set; }
    public JsonElement? Payload { get; set; }
}

internal sealed class Reply
{
    public string? Id { get; set; }
    public bool Ok { get; set; }
    public string? Data { get; set; }
    public string? Error { get; set; }

    public static Reply Success(string? id, object data) => new()
    {
        Id = id,
        Ok = true,
        Data = JsonSerializer.Serialize(data),
    };

    public static Reply Failure(string? id, string error) => new()
    {
        Id = id,
        Ok = false,
        Error = error,
    };
}

/// <summary>
/// The operations the UWP side may ask for. Deliberately a closed list, not a shell: the
/// companion is reachable only from this package, but "run any command" is still a capability
/// nothing in the product needs yet.
/// </summary>
internal static class Ops
{
    public static async Task<object> RunAsync(Request request)
    {
        return request.Op switch
        {
            "ping" => Ping(),
            "env" => Env(),
            "echo" => request.Payload?.ToString() ?? "",
            "probe" => await ProbeAsync().ConfigureAwait(false),
            _ => throw new InvalidOperationException($"未知操作 `{request.Op}`"),
        };
    }

    private static object Ping()
    {
        string? family = null;
        string? fullName = null;
        bool packaged;
        try
        {
            family = Windows.ApplicationModel.Package.Current.Id.FamilyName;
            fullName = Windows.ApplicationModel.Package.Current.Id.FullName;
            packaged = true;
        }
        catch
        {
            packaged = false;
        }
        return new
        {
            version = "0.1.0",
            pid = Environment.ProcessId,
            packaged,
            packageFamilyName = family,
            packageFullName = fullName,
            os = Environment.OSVersion.VersionString,
        };
    }

    private static object Env() => new
    {
        appData = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        temp = System.IO.Path.GetTempPath(),
        cwd = Environment.CurrentDirectory,
        user = Environment.UserName,
    };

    /// <summary>
    /// Runs `dsh --version` and reports what actually happened. This is the smallest possible
    /// proof that the full-trust half can start a child process at all — the capability the
    /// AppContainer does not have.
    /// </summary>
    private static async Task<object> ProbeAsync()
    {
        var executable = FindDsh();
        if (executable is null)
        {
            return new
            {
                found = false,
                command = "dsh --version",
                hint = "PATH 里没有 dsh；先 saiku runtime install 或把 dsh 放进 PATH",
            };
        }

        var start = new ProcessStartInfo
        {
            FileName = executable,
            Arguments = "--version",
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true,
        };
        try
        {
            using var process = Process.Start(start)!;
            var stdout = process.StandardOutput.ReadToEndAsync();
            var stderr = process.StandardError.ReadToEndAsync();
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(20));
            try
            {
                await process.WaitForExitAsync(timeout.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                try { process.Kill(entireProcessTree: true); } catch { }
                return new { found = true, command = "dsh --version", timedOut = true, exitCode = -1 };
            }
            return new
            {
                found = true,
                command = "dsh --version",
                exitCode = process.ExitCode,
                stdout = (await stdout.ConfigureAwait(false)).Trim(),
                stderr = (await stderr.ConfigureAwait(false)).Trim(),
            };
        }
        catch (Exception ex)
        {
            return new
            {
                found = true,
                command = "dsh --version",
                error = $"{ex.GetType().Name}: {ex.Message}",
            };
        }
    }

    private static string? FindDsh()
    {
        var paths = (Environment.GetEnvironmentVariable("PATH") ?? "")
            .Split(System.IO.Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries);
        foreach (var directory in paths)
        {
            foreach (var name in new[] { "dsh.exe", "dsh.cmd", "dsh.bat" })
            {
                try
                {
                    var candidate = System.IO.Path.Combine(directory.Trim('"'), name);
                    if (File.Exists(candidate)) return candidate;
                }
                catch
                {
                    // A malformed PATH entry is not a reason to fail the probe.
                }
            }
        }
        return null;
    }
}
