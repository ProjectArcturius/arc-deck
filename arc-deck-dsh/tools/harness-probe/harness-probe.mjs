// Remote 层探针 —— 「直接连 harness 内核」这条路的可行性验证。
//
// 动机：SDK stdio 那条路只能 create-session 然后 prompt，**拿不到会话列表、
// 拿不到历史、不能改名**。而 harness 自己那份 WebView 客户端走的是
// **Remote 层**（`/api` HTTP + `/api/remote.mux` WebSocket），那才是完整功能面。
//
// 协议形状是从 harness 源码读出来的（`packages/client/connection/src/`、
// `packages/api/gateway/src/`），本脚本用最小实现把它逐条验证一遍：
//
//   1. 能不能起一个 harness web 内核并从 stdout 里拿到带 token 的 URL
//   2. GET /?token=… → Set-Cookie（浏览器会话），这是唯一的换票口
//   3. POST /api/<ns>/<method> → { type:'server-response', rpcId, result }
//   4. WS /api/remote.mux → open/item/error/end 逻辑流
//
// 用法：
//   node harness-probe.mjs spawn           # 起内核 → 登录 → session/list
//   node harness-probe.mjs spawn follow <sessionId>
//
// 为什么先用 Node 而不是直接写 C#：这三件事（换票、RPC 信封、WS 帧）任何
// 一处猜错，在 C# 里都要重编译才能发现；在这里是改一行再看。

import { spawn } from 'node:child_process'
import { createServer } from 'node:net'
import { existsSync } from 'node:fs'
import { randomUUID } from 'node:crypto'

const DSH_ROOT = process.env.ARC_DECK_DSH_ROOT
  ?? 'C:\\Users\\mc158\\AppData\\Local\\Programs\\DSH Desktop'
const APP_ROOT = `${DSH_ROOT}\\resources\\app`
const NODE = `${APP_ROOT}\\node_modules\\node\\bin\\node.exe`
const DSH_ENTRY = `${APP_ROOT}\\node_modules\\@deepseek-ai\\dsh\\lib\\bin.js`
const HARNESS_HOME = process.env.DSH_HOME
  ?? 'C:\\Users\\mc158\\AppData\\Roaming\\dsh-desktop\\harness'

for (const p of [NODE, DSH_ENTRY]) {
  if (!existsSync(p)) {
    console.error(`找不到 ${p}`)
    process.exit(2)
  }
}

const mode = process.argv[2] ?? 'spawn'
const followTarget = process.argv[3]

/** 要一个空闲端口 —— 用 0 让内核自己选也行，但那样还得从 stdout 里再解析端口。 */
function freePort() {
  return new Promise((resolve, reject) => {
    const s = createServer()
    s.listen(0, '127.0.0.1', () => {
      const { port } = s.address()
      s.close(() => resolve(port))
    })
    s.on('error', reject)
  })
}

const port = await freePort()
console.log(`启动 harness web（port=${port}）…`)

const child = spawn(NODE, [DSH_ENTRY, 'web', '--no-open', '--host', '127.0.0.1', '--port', String(port)], {
  cwd: HARNESS_HOME,
  env: { ...process.env, DSH_HOME: HARNESS_HOME },
  stdio: ['ignore', 'pipe', 'pipe'],
  windowsHide: true,
})

let stdout = ''
let tokenUrl = null
const tokenSeen = new Promise((resolve) => {
  const scan = (chunk) => {
    stdout += chunk.toString('utf8')
    if (tokenUrl) return
    const m = stdout.match(/https?:\/\/127\.0\.0\.1:\d+\/\?token=[A-Za-z0-9_-]+/)
    if (m) {
      tokenUrl = m[0]
      // 只把**去掉 token 的那部分**打出来：这一行的用途是证明「拿到了票」，
      // 把票本身写进日志等于把凭据落盘。
      console.log(`✓ 从 stdout 拿到带 token 的 URL（${m[0].split('?')[0]}/?token=<已隐去>）`)
      resolve(m[0])
    }
  }
  child.stdout.on('data', scan)
  // 有的版本把 URL 写在 stderr 上，两处都扫。
  child.stderr.on('data', (b) => {
    const s = b.toString('utf8')
    if (s.includes('token=')) scan(b)
    else process.stderr.write(`[dsh] ${s}`)
  })
})

const timeout = new Promise((_, rej) => setTimeout(() => rej(new Error('等待 token 超时（90s）')), 90_000))

async function shutdown(code) {
  try { child.kill() } catch { }
  process.exit(code)
}

process.on('SIGINT', () => shutdown(130))

let url
try {
  url = await Promise.race([tokenSeen, timeout])
} catch (e) {
  console.error(`✗ ${e.message}`)
  console.error(stdout.slice(-2000))
  await shutdown(2)
}

const origin = new URL(url).origin

// ── 2. 换票：GET /?token=… 拿到 Set-Cookie ───────────────────────────────
console.log('\n── 换票 ──')
const rootRes = await fetch(url, { redirect: 'manual' })
const setCookie = rootRes.headers.getSetCookie?.() ?? []
console.log(`GET /?token=… → ${rootRes.status} ${rootRes.headers.get('location') ?? ''}`)
console.log(`  Set-Cookie: ${setCookie.map(c => c.split(';')[0]).join(' | ') || '(无)'}`)

const cookie = setCookie.map(c => c.split(';')[0]).join('; ')
if (!cookie) {
  console.error('✗ 没拿到 cookie —— 后面所有 /api 都会被 401')
  await shutdown(2)
}

// 换票之后再访问干净路径，验证 cookie 真的生效（这正是浏览器做的事）。
const clean = await fetch(`${origin}/`, { headers: { cookie } })
console.log(`GET / （带 cookie）→ ${clean.status}`)

// ── 3. RPC 信封 ─────────────────────────────────────────────────────────
async function call(namespace, method, args) {
  const rpcId = randomUUID()
  const body = { type: 'client-request', rpcId, method: `${namespace}/${method}`, payload: { args } }
  const res = await fetch(`${origin}/api/${namespace}/${method}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', cookie },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`HTTP ${res.status} ${await res.text()}`)
  const frame = await res.json()
  if (frame.rpcId !== rpcId) throw new Error(`rpcId 不匹配：发了 ${rpcId}，收到 ${frame.rpcId}`)
  return frame.result
}

console.log('\n── session/list ──')
// **参数名必须与 Host 签名逐字一致** —— 实测第一次用 `{}` 被拒：
//
//   gateway/arguments-invalid: session/list: args fields do not match
//   the descriptor: missing "_request"
//
// Gateway 要求 args 的字段集合与描述符完全一致（参 docs/api-gateway.zh.md
// 「运行时调用」）。所以这一层不能靠猜，要按源码里的形参名写。
const list = await call('session', 'list', { _request: {} })
if (!list.ok) {
  console.error(`✗ ${list.error.code}: ${list.error.message}`)
} else {
  const items = list.value.items
  console.log(`✓ ${items.length} 个会话`)
  for (const s of items.slice(0, 5)) {
    console.log(`  ${s.sessionId}  running=${s.running}  blank=${s.blank}  cwd=${s.cwd ?? '(无)'}`)
  }
}

// ── 4. 逻辑流 ───────────────────────────────────────────────────────────
async function follow(sessionId) {
  console.log(`\n── session/follow（WebSocket）──`)
  // 浏览器 WebSocket 不能带自定义头，只能靠 Cookie 头（Node 支持）。
  // 这也正是 C# 侧要照做的方式：把换票拿到的 cookie 放进 Upgrade 请求头。
  const ws = new WebSocket(`${origin.replace('http', 'ws')}/api/remote.mux`, {
    headers: { Cookie: cookie },
  })

  const streamId = randomUUID()
  let items = 0

  ws.addEventListener('open', () => {
    console.log('✓ WebSocket 已开')
    ws.send(JSON.stringify({
      type: 'open',
      streamId,
      endpoint: 'session/follow',
      // 同样按 Host 形参名：`follow(request, signal)` → wire 字段 `request`
      // （`signal` 记在描述符里，不进 args）。
      payload: { args: { request: { address: { kind: 'session', sessionId }, maxMessages: 50, assistantStream: true } } },
    }))
  })

  ws.addEventListener('message', (ev) => {
    const frame = JSON.parse(String(ev.data))
    if (frame.streamId !== streamId) return
    if (frame.type === 'error') {
      console.error(`✗ 流错误 ${frame.error.code}: ${frame.error.message}`)
      ws.close()
      return
    }
    if (frame.type === 'end') {
      console.log('流结束')
      ws.close()
      return
    }
    items++
    const v = frame.value
    if (items <= 3 || items % 200 === 0) {
      if (v?.type === 'snapshot') {
        console.log(`  [snapshot] ${v.records.length} 条历史 · cursor=${v.cursor} · hasMore=${v.hasMore} · header.id=${v.header?.id}`)
      } else if (v?.type === 'event') {
        console.log(`  [event] seq=${v.event.seq} ${v.event.type}${v.event.ignorable ? ' (ignorable)' : ''}`)
      } else {
        console.log(`  [${v?.type}]`)
      }
    }
  })

  ws.addEventListener('error', () => console.error('✗ WebSocket 错误'))
  ws.addEventListener('close', (e) => {
    console.log(`WebSocket 关闭 code=${e.code} 共收到 ${items} 个 item`)
    shutdown(0)
  })

  setTimeout(() => {
    console.log(`（12s 到，主动关闭；共 ${items} 个 item）`)
    ws.close()
  }, 12_000)
}

if (mode === 'spawn' && followTarget) {
  await follow(followTarget)
} else {
  await shutdown(0)
}
