// R5 可行性探针 —— 「直接连已经跑着的 dsh 内核」这条路到底通不通。
//
// 动机：UWP 不能起子进程，所以 stdio 那条路（dsh --profile sdk）在 UWP 里走不通。
// 剩下唯一不需要起进程的接法是连上**已经在跑的**内核的 WebSocket。
// 那个 WebSocket 是浏览器 UI 用的私有通道，没有文档，所以先用最短的探针
// 把三件事问清楚，而不是先写 C# 再发现走不通：
//
//   1. 握手要不要鉴权（没 token 能不能连上）
//   2. 服务端第一帧长什么样
//   3. 端点名（`/api/...`）怎么枚举
//
// 用法：
//   node arc-deck-probe.mjs ws            # 连 ws://127.0.0.1:43129/api/remote.mux
//   node arc-deck-probe.mjs ws <url>      # 指定地址
//   node arc-deck-probe.mjs http          # 列 HTTP 路由的鉴权行为

const mode = process.argv[2] ?? "ws";
const base = process.argv[3] ?? process.env.DSH_WEB_URL ?? "http://127.0.0.1:43129";

if (mode === "http") {
  const paths = ["/", "/index.html", "/api", "/api/remote.mux", "/healthz", "/api/session"];
  for (const p of paths) {
    try {
      const r = await fetch(new URL(p, base), { redirect: "manual" });
      const body = (await r.text()).slice(0, 200).replaceAll("\n", " ");
      console.log(`${p.padEnd(18)} ${r.status} ${r.headers.get("content-type") ?? ""} ${body}`);
    } catch (e) {
      console.log(`${p.padEnd(18)} ERROR ${e.message}`);
    }
  }
  process.exit(0);
}

const wsUrl = process.argv[3] ?? base.replace(/^http/, "ws") + "/api/remote.mux";
console.log(`连接 ${wsUrl}（不带 token）`);

const socket = new WebSocket(wsUrl);

socket.addEventListener("open", () => {
  console.log("✓ 握手成功 —— 本地回环不需要 token");
  // 试着发一个明显不存在的端点，看错误帧里会不会透出可用端点的线索。
  socket.send(JSON.stringify({
    streamId: "probe-1",
    endpoint: "/api/__probe__",
    args: {},
  }));
  console.log("→ 已发 probe 帧");
});

socket.addEventListener("message", (event) => {
  console.log(`← ${String(event.data).slice(0, 500)}`);
});

socket.addEventListener("error", (event) => {
  console.log(`✗ error ${event.message ?? "(no message)"}`);
});

socket.addEventListener("close", (event) => {
  console.log(`关闭 code=${event.code} reason=${event.reason || "(none)"}`);
  process.exit(0);
});

setTimeout(() => {
  console.log("（超时 8s，主动关闭）");
  socket.close();
}, 8000);
