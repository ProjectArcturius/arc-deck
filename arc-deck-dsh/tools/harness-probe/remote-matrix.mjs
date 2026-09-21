// Harness Remote 层的**功能矩阵**验证 —— 界面要用到的每一条先在这里跑通。
//
// 与 `harness-probe.mjs` 的分工：那个是「协议通不通」，这个是「功能全不全」。
// 两条都值得单独存在，因为它们的失败原因不同 —— 协议错了是 401/参数校验，
// 功能错了是「某个方法在 0.1.6 上改了形状」。
//
// 覆盖：
//   1. session/list          列会话（真实数据，不是磁盘扫描）
//   2. session/page          向后分页取完整历史（follow 的 snapshot 只有一页）
//   3. session/follow        live 流
//   4. session/prompt        发消息
//   5. session/control       jobs 与实时控制 baseline
//   6. modelCatalog          provider/model 目录（界面上的模型选择器）
//
// 用法：
//   node remote-matrix.mjs                 # 除 prompt 外全跑（只读）
//   node remote-matrix.mjs --prompt "你好"  # 连 prompt 一起跑
import { spawn } from 'node:child_process'
import { createServer } from 'node:net'
import { randomUUID } from 'node:crypto'

const DSH_ROOT = process.env.ARC_DECK_DSH_ROOT
  ?? 'C:\\Users\\mc158\\AppData\\Local\\Programs\\DSH Desktop'
const APP = `${DSH_ROOT}\\resources\\app`
const NODE = `${APP}\\node_modules\\node\\bin\\node.exe`
const ENTRY = `${APP}\\node_modules\\@deepseek-ai\\dsh\\lib\\bin.js`
const HOME = process.env.DSH_HOME ?? 'C:\\Users\\mc158\\AppData\\Roaming\\dsh-desktop\\harness'

const promptText = process.argv.includes('--prompt')
  ? process.argv[process.argv.indexOf('--prompt') + 1]
  : null

const results = []
const ok = (name, detail) => { results.push(['✓', name, detail]); console.log(`✓ ${name} — ${detail}`) }
const bad = (name, detail) => { results.push(['✗', name, detail]); console.log(`✗ ${name} — ${detail}`) }

// ── 起内核 + 换票 ───────────────────────────────────────────────────────
const port = await new Promise((res) => {
  const s = createServer()
  s.listen(0, '127.0.0.1', () => { const p = s.address().port; s.close(() => res(p)) })
})

const child = spawn(NODE, [ENTRY, 'web', '--no-open', '--host', '127.0.0.1', '--port', String(port)],
  { cwd: HOME, env: { ...process.env, DSH_HOME: HOME }, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true })

let out = ''
const url = await new Promise((resolve, reject) => {
  const t = setTimeout(() => reject(new Error('等 token 超时')), 90_000)
  const scan = (b) => {
    out += b.toString('utf8')
    const m = out.match(/https?:\/\/127\.0\.0\.1:\d+\/\?token=[A-Za-z0-9_-]+/)
    if (m) { clearTimeout(t); resolve(m[0]) }
  }
  child.stdout.on('data', scan)
  child.stderr.on('data', scan)
})

const origin = new URL(url).origin
const cookie = (await fetch(url, { redirect: 'manual' })).headers.getSetCookie()
  .map(c => c.split(';')[0]).join('; ')

async function call(namespace, method, args) {
  const rpcId = randomUUID()
  const res = await fetch(`${origin}/api/${namespace}/${method}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', cookie },
    body: JSON.stringify({ type: 'client-request', rpcId, method: `${namespace}/${method}`, payload: { args } }),
  })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  const frame = await res.json()
  if (frame.rpcId !== rpcId) throw new Error('rpcId 不匹配')
  if (!frame.result.ok) throw new Error(`${frame.result.error.code}: ${frame.result.error.message}`)
  return frame.result.value
}

/** 开一条逻辑流，收满 n 个 item 或超时就关，返回收集到的值。 */
function openStream(endpoint, args, { collect = 50, ms = 6000 } = {}) {
  return new Promise((resolve) => {
    const ws = new WebSocket(`${origin.replace('http', 'ws')}/api/remote.mux`, { headers: { Cookie: cookie } })
    const streamId = randomUUID()
    const values = []
    let error = null
    const done = () => {
      try { ws.close() } catch { }
      resolve({ values, error })
    }
    ws.addEventListener('open', () => ws.send(JSON.stringify({ type: 'open', streamId, endpoint, payload: { args } })))
    ws.addEventListener('message', (ev) => {
      const f = JSON.parse(String(ev.data))
      if (f.streamId !== streamId) return
      if (f.type === 'error') { error = `${f.error.code}: ${f.error.message}`; done(); return }
      if (f.type === 'end') { done(); return }
      values.push(f.value)
      if (values.length >= collect) done()
    })
    ws.addEventListener('error', () => { error = 'WebSocket error'; done() })
    setTimeout(done, ms)
  })
}

// ── 1. session/list ─────────────────────────────────────────────────────
let sessions = []
try {
  const v = await call('session', 'list', { _request: {} })
  sessions = v.items
  ok('session/list', `${sessions.length} 个会话，含 running/cwd/updatedAt`)
} catch (e) { bad('session/list', e.message) }

// 挑一个「有内容、不是 blank」的会话做后面的测试。
//
// **实测踩到**：某个会话的日志会报
// `corrupt Zstandard session log: complete frame contains a torn JSONL record`
// —— 那多半是「一段正在被追加的压缩帧」。所以这里不押在一个会话上：
// 按「最近活动」排，逐个试到能 follow 的为止。这本身也是界面要做的事：
// **一个会话打不开不该让整个列表打不开**。
const candidates = sessions.filter(s => !s.blank).slice(0, 8)
if (candidates.length === 0) { console.log('没有可用会话，停'); child.kill(); process.exit(1) }
console.log(`候选会话 ${candidates.length} 个，先用 ${candidates[0].sessionId}\n`)

// ── 2. session/follow（snapshot + live） ────────────────────────────────
let followResult = null
let target = null
for (const candidate of candidates) {
  const r = await openStream('session/follow',
    { request: { address: { kind: 'session', sessionId: candidate.sessionId }, maxMessages: 50 } },
    { collect: 999999, ms: 5000 })
  const snap = r.values.find(v => v?.type === 'snapshot')
  if (snap) {
    followResult = r
    target = candidate
    ok('session/follow', `${candidate.sessionId} · snapshot ${snap.records.length} 条 · cursor=${snap.cursor}` +
      ` · hasMore=${snap.hasMore} · 后续 live ${r.values.length - 1} 个`)
    break
  }
  console.log(`  · ${candidate.sessionId}: ${r.error ?? '无 snapshot'}`)
}

if (!target) {
  bad('session/follow', `试了 ${candidates.length} 个会话都打不开`)
} else {
  console.log(`  目标会话：${target.sessionId}\n`)
}

// ── 3. session/page 向后翻到完整历史 ────────────────────────────────────
try {
  const snap = followResult?.values.find(v => v?.type === 'snapshot')
  let beforeSeq = snap?.records?.[0]?.event?.seq
  const throughSeq = snap?.cursor
  let pages = 0
  let total = snap?.records?.length ?? 0
  while (beforeSeq !== undefined && pages < 20) {
    const page = await call('session', 'page',
      { request: { address: { kind: 'session', sessionId: target.sessionId }, throughSeq, beforeSeq, maxMessages: 200 } })
    pages++
    total += page.records.length
    if (!page.hasMore || page.records.length === 0) break
    beforeSeq = page.records[0]?.event?.seq
  }
  ok('session/page', `${pages} 页，累计 ${total} 条历史记录（含 snapshot）`)
} catch (e) { bad('session/page', e.message) }

// ── 4. session/control（jobs baseline） ────────────────────────────────
try {
  const r = await openStream('session/control', {}, { collect: 5, ms: 4000 })
  const first = r.values[0]
  if (!first) throw new Error(r.error ?? '没有 baseline')
  ok('session/control', `baseline keys=[${Object.keys(first).join(',')}] jobs=${Object.keys(first.jobs ?? {}).length} 个会话有 job`)
} catch (e) { bad('session/control', e.message) }

// ── 5. modelCatalog ────────────────────────────────────────────────────
try {
  const v = await call('session', 'modelCatalog', {})
  const groups = v.groups ?? v.providers ?? []
  const names = groups.map(g => `${g.id ?? g.name}(${(g.models ?? []).length})`).join(' ')
  ok('session/modelCatalog', `${groups.length} 个 provider：${names}`)
} catch (e) { bad('session/modelCatalog', e.message) }

// ── 6. session/prompt（可选，会真的发消息） ─────────────────────────────
if (promptText) {
  try {
    const requestId = randomUUID()
    const v = await call('session', 'prompt', {
      request: {
        requestId,
        sessionId: target.sessionId,
        mode: 'queue',
        content: [{ type: 'text', text: promptText }],
      },
    })
    ok('session/prompt', `accepted=${v.accepted}`)

    // 发完之后看 follow 是否收到新的 live 事件 —— 这才叫「真的通了」。
    const after = await openStream('session/follow',
      { request: { address: { kind: 'session', sessionId: target.sessionId }, maxMessages: 5 } },
      { collect: 999999, ms: 25_000 })
    const types = {}
    for (const v2 of after.values) {
      if (v2?.type === 'event') types[v2.event.type] = (types[v2.event.type] ?? 0) + 1
    }
    const entries = Object.entries(types)
    if (entries.length === 0) bad('prompt 之后的 live 事件', '一个事件都没收到')
    else ok('prompt 之后的 live 事件', entries.map(([k, n]) => `${k}×${n}`).join(' '))
  } catch (e) { bad('session/prompt', e.message) }
}

console.log('\n════ 汇总 ════')
for (const [mark, name, detail] of results) console.log(`${mark} ${name}: ${detail}`)

child.kill()
process.exit(results.some(r => r[0] === '✗') ? 1 : 0)
