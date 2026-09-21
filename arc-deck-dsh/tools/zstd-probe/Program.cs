// 一次性 API 探针：确认 ZstdSharp 能读**多帧拼接**的 zstd 流。
//
// 为什么必须验证这一点而不是直接信文档：harness 的会话日志是**追加写**的
// （`session.jsonl.zstd`），每追加一段就是一个新的 zstd 帧。多数解压 API
// 只解第一帧就报成功 —— 那样会**静默丢掉会话后半段**，而表现是
// 「重载出来的会话少了最近的几条消息」，看起来像投影逻辑的 bug。
//
// 判据是**与 zstd 命令行逐字节一致**，不是「没抛异常」。
//
// 用法：
//   dotnet run --project arc-deck-dsh/tools/zstd-probe -- <file.jsonl.zstd> [期望解压字节数]
using System.Diagnostics;
using ZstdSharp;

if (args.Length == 0)
{
    Console.WriteLine("用法: zstd-probe <session.jsonl.zstd> [期望解压字节数]");
    Console.WriteLine("（给 - 时从 stdin 读）");
    return 2;
}

byte[] bytes;
string label;
if (args[0] == "-")
{
    using var stdin = Console.OpenStandardInput();
    using var buffer = new MemoryStream();
    stdin.CopyTo(buffer);
    bytes = buffer.ToArray();
    label = "stdin";
}
else
{
    bytes = File.ReadAllBytes(args[0]);
    label = args[0];
}

var expected = args.Length > 1 ? long.Parse(args[1]) : -1L;
Console.WriteLine($"输入 {label}");
Console.WriteLine($"  压缩后 {bytes.Length:N0} 字节");

var sw = Stopwatch.StartNew();
using var input = new MemoryStream(bytes);

// checkEndOfStream:false 是刻意的：拼接流的“末尾”由我们自己判断，
// 让第一帧的帧尾就抛错会把一份完好的日志判成坏的。
using var decompression = new DecompressionStream(input, bufferSize: 1 << 16, checkEndOfStream: false);
using var output = new MemoryStream();
decompression.CopyTo(output);
sw.Stop();

Console.WriteLine($"  解压后 {output.Length:N0} 字节（{sw.ElapsedMilliseconds} ms）");
if (expected >= 0)
{
    Console.WriteLine(output.Length == expected
        ? $"  ✓ 与 zstd 命令行逐字节一致（{expected:N0}）"
        : $"  ✗ 与期望不一致：{output.Length:N0} ≠ {expected:N0}");
}

var text = System.Text.Encoding.UTF8.GetString(output.ToArray());
var lines = text.Split('\n', StringSplitOptions.RemoveEmptyEntries);
Console.WriteLine($"  行数 {lines.Length:N0}");
if (lines.Length > 0)
{
    Console.WriteLine($"  首行 {Shorten(lines[0])}");
    Console.WriteLine($"  末行 {Shorten(lines[^1])}");
}

return expected < 0 || output.Length == expected ? 0 : 1;

static string Shorten(string s) => s.Length <= 120 ? s : s[..120] + "…";
