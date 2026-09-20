// dsh SDK stdio 探针 —— 用来在没有 C# 侧的情况下确认协议通路。
//
// 为什么先写这个：C# 客户端一旦建起来，排查「没消息」时无法区分
// 「进程没起来」「帧格式不对」「事件到了没翻译」。先在 node 里把上游
// 的真实帧打出来，C# 侧就有一个可对照的基线。
//
// 用法：
//   node arc-deck/tools/sdk-probe.mjs "<提示词>" [工作目录]
//
// 需要环境里能找到 dsh 的安装位置；默认取 DSH Desktop 自带的副本。

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { randomUUID } from "node:crypto";

const DSH_BIN = process.env.DSH_SDK_BIN
  ?? "C:\\Users\\mc158\\AppData\\Local\\Programs\\DSH Desktop\\resources\\app\\node_modules\\@deepseek-ai\\dsh\\lib\\bin.js";

const prompt = process.argv[2] ?? "只回复两个字：收到";
const cwd = process.argv[3] ?? process.cwd();

if (!existsSync(DSH_BIN)) {
  console.error(`找不到 dsh bin：${DSH_BIN}`);
  process.exit(2);
}

const child = spawn(process.execPath, [DSH_BIN, "--profile", "sdk"], {
  cwd,
  stdio: ["pipe", "pipe", "pipe"],
  windowsHide: true,
});

child.stderr.on("data", (b) => process.stderr.write(`[stderr] ${b}`));

let buffer = "";
let nextId = 0;
const pending = new Map();

child.stdout.on("data", (b) => {
  buffer += b.toString("utf8");
  for (;;) {
    const nl = buffer.indexOf("\n");
    if (nl < 0) break;
    const line = buffer.slice(0, nl).trim();
    buffer = buffer.slice(nl + 1);
    if (!line) continue;

    let frame;
    try { frame = JSON.parse(line); } catch {
      console.log(`[非 JSON 行] ${line.slice(0, 200)}`);
      continue;
    }

    if (frame.id !== undefined && frame.method === undefined) {
      const waiter = pending.get(frame.id);
      pending.delete(frame.id);
      if (frame.error) {
        console.log(`[响应 ${waiter?.method}] error ${JSON.stringify(frame.error)}`);
        waiter?.reject(new Error(frame.error.message));
      } else {
        console.log(`[响应 ${waiter?.method}] ${JSON.stringify(frame.result)}`);
        waiter?.resolve(frame.result);
      }
      continue;
    }

    // 通知：只把方法名和关键字段打出来，避免刷屏
    if (frame.method === "session.event") {
      const e = frame.params?.event ?? {};
      console.log(`[事件] ${e.type ?? "?"} ${summarize(e)}`);
    } else if (frame.method === "session.status") {
      console.log(`[状态] ${frame.params?.status ?? JSON.stringify(frame.params)}`);
    } else {
      console.log(`[通知] ${frame.method} ${JSON.stringify(frame.params).slice(0, 400)}`);
    }
  }
});

function summarize(event) {
  switch (event.type) {
    case "assistant/chunk":
      return JSON.stringify(event.chunk ?? event).slice(0, 160);
    case "assistant/message":
      return JSON.stringify(event.message?.content ?? event).slice(0, 200);
    case "tool/call":
      return `${event.name ?? "?"} ${JSON.stringify(event.arguments ?? {}).slice(0, 120)}`;
    case "turn/start":
    case "turn/end":
      return "";
    default:
      return JSON.stringify(event).slice(0, 200);
  }
}

function request(method, params) {
  const id = `req_${nextId++}_${randomUUID().slice(0, 8)}`;
  const frame = { jsonrpc: "2.0", id, method, params };
  console.log(`[请求 ${method}] ${JSON.stringify(params)}`);
  child.stdin.write(`${JSON.stringify(frame)}\n`);
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject, method });
  });
}

const sessionId = `probe-${randomUUID().slice(0, 8)}`;

try {
  await request("initialize", {
    cwd,
    provider: "deepseek-official",
    model: "deepseek-official",
  });

  await request("session/prompt", {
    sessionId,
    contentBlocks: [{ type: "text", text: prompt }],
  });

  // 等到 agent 回到 idle，或超时。
  await new Promise((resolve) => setTimeout(resolve, 90_000));
} finally {
  try { await request("shutdown", {}); } catch { /* 已在退出 */ }
  child.stdin.end();
  setTimeout(() => { child.kill(); process.exit(0); }, 1500);
}
