using System.Text.Json;

namespace ArcDeck.Host;

/// <summary>
/// Entry point of the full-trust companion. Two modes:
///
///   (no args)     connect to the UWP app's `arcdeck.host` AppService and serve requests
///   --selftest    run the same operations locally and print one JSON line per op
///
/// `--selftest` exists so the operations can be tested without a package, a deployment or a
/// UWP instance — the deployment path is the expensive one and should be exercised only for
/// the things that genuinely need it.
/// </summary>
internal static class Program
{
    [STAThread]
    private static int Main(string[] args)
    {
        HostLog.Write("start: " + string.Join(" ", args));
        try
        {
            if (args.Contains("--selftest"))
            {
                return SelfTest().GetAwaiter().GetResult();
            }

            var service = Argument(args, "--service") ?? "arcdeck.host";
            return AppServiceClient.RunAsync(service).GetAwaiter().GetResult();
        }
        catch (Exception ex)
        {
            HostLog.Write("fatal: " + ex);
            return 2;
        }
    }

    private static async Task<int> SelfTest()
    {
        var ops = new[] { "ping", "env", "echo", "probe" };
        foreach (var op in ops)
        {
            var request = new Request
            {
                Id = op,
                Op = op,
                Payload = op == "echo" ? JsonSerializer.SerializeToElement("hello") : null,
            };
            try
            {
                var data = await Ops.RunAsync(request).ConfigureAwait(false);
                Console.WriteLine(JsonSerializer.Serialize(new { id = op, ok = true, data }));
            }
            catch (Exception ex)
            {
                Console.WriteLine(JsonSerializer.Serialize(new { id = op, ok = false, error = ex.Message }));
            }
        }
        return 0;
    }

    private static string? Argument(string[] args, string name)
    {
        var index = Array.IndexOf(args, name);
        if (index < 0 || index + 1 >= args.Length) return null;
        return args[index + 1];
    }
}
