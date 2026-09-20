# Arc Deck · DSH 协议客户端

这是 **Arc Deck 两个实现共用的那一层**：说漏了这句话会让人以为它可以随便搬。

```
arc-deck-dsh/          ← 本目录：协议客户端（.NET 9，桌面进程）
    Transport/         stdio 行分帧 JSON-RPC
    Protocol/          方法名、帧结构、事件翻译
    Runtime/           找 dsh 安装、准备 profile、起进程
uwp/src/               UWP 界面（AppContainer）
kanesumi/crates/       Rust 界面（将来的 Wayland/Win32 自绘外壳）
```

## 为什么它是一个**独立进程**

UWP 版的 `ArcDeck.csproj` 里写着 `<OutputType>AppContainerExe</OutputType>` ——
那是 AppContainer，**不能起子进程**，也不能读 AppContainer 之外的文件。
而 dsh 内核就是一个外部 Node 进程，必须由客户端起。

所以分工是：

```
Arc Deck 界面  (AppContainer)  ──IPC──▶  arc-deck-dsh host (完整信任)  ──stdio──▶  node dsh --profile sdk
```

**不要**把协议解析塞回 UWP 工程里「省一层」：AppContainer 那一侧永远拿不到
进程句柄，省下来的那一层最后会变成一整套重写的通信代码。

## 协议

上游是 `@deepseek-ai/dsh-sdk-protocol`：按 `\n` 分帧的 JSON-RPC 2.0，
方法只有三个（`initialize` / `session/prompt` / `shutdown`），通知有四个。

**上游协议没有版本协商**（`serverInfo.version` 恒为 `0.0.1`，客户端不校验），
所以 `DshVersion.cs` 把对侧的版本钉死；升级上游是刻意的工程动作，不是自动跟随。

## 跑

```powershell
dotnet run --project arc-deck-dsh/src/ArcDeck.Dsh.Cli -- --prompt "只回复两个字：收到"
```
