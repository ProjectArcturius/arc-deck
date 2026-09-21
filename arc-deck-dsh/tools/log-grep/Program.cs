// 会话日志里某一类事件的**原文**取样器。
//
// 为什么单独一个工具：`--type` 那个摘要视图只打它认识的字段（turn/step/…），
// 而 tool/result 这种载荷的信息全在 `data.message.content[0]` 里 ——
// 要判断「工具到底跑了没有 / 被审批拒了没有」，必须看原文。
//
// 用法：
//   dotnet run --project arc-deck-dsh/tools/log-grep -- <session.jsonl.zstd> <事件类型> [帧数]
using System.Text.Json.Nodes;
using ArcDeck.Dsh.Sessions;

if (args.Length < 2)
{
    Console.WriteLine("用法: log-grep <session.jsonl.zstd> <事件类型> [帧数=3]");
    return 2;
}

var take = args.Length > 2 ? int.Parse(args[2]) : 3;
var text = DshSessionLogFile.ReadText(args[0]);
var split = DshSessionLogFile.SplitLines(text);
var shown = 0;

foreach (var line in split.Lines)
{
    JsonNode? node;
    try { node = JsonNode.Parse(line); } catch { continue; }
    if (node is not JsonObject obj) continue;
    if (obj["type"]?.GetValue<string>() != args[1]) continue;

    Console.WriteLine(obj.ToJsonString(new System.Text.Json.JsonSerializerOptions
    {
        WriteIndented = true,
        Encoder = System.Text.Encodings.Web.JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
    }));
    Console.WriteLine();

    if (++shown >= take) break;
}

Console.WriteLine($"（共显示 {shown} 帧）");
return shown > 0 ? 0 : 1;
