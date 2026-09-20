import { spawn } from "node:child_process"

const BERTH_BIN = __AGENT_BERTH_BIN__

export default function (pi) {
  const seen = new Set()
  const running = new Set()
  const pending = new Set()
  const names = new Map()
  let heartbeat
  let inflight = false

  function sid(ctx) {
    const id = ctx.sessionManager.getSessionId()
    if (typeof id === "string" && id) return id
    const file = ctx.sessionManager.getSessionFile()
    if (typeof file === "string" && file) return file
    return String(process.pid)
  }

  function prune(keep) {
    for (const id of [...seen]) {
      if (id === keep || running.has(id) || pending.has(id)) continue
      seen.delete(id)
      names.delete(id)
    }
  }

  function firstMessage(ctx) {
    for (const entry of ctx.sessionManager.getEntries?.() ?? []) {
      if (entry?.message?.role !== "user") continue
      return messageText(entry.message)
    }
    return ""
  }

  function remember(ctx, prompt) {
    const id = sid(ctx)
    const name = pi.getSessionName?.()
    if (typeof name === "string" && name) {
      names.set(id, name)
      return
    }
    if (names.has(id)) return
    const text = String(prompt ?? firstMessage(ctx)).replace(/\s+/g, " ").trim()
    if (text) names.set(id, text)
  }

  function messageText(message) {
    const content = message?.content
    if (typeof content === "string") return content
    if (!Array.isArray(content)) return ""
    return content
      .filter((block) => block?.type === "text")
      .map((block) => block.text)
      .join(" ")
  }

  function flush() {
    if (inflight) return
    inflight = true
    const status = {}
    for (const id of seen) status[id] = running.has(id) ? "busy" : "idle"
    const titles = {}
    for (const id of seen) {
      const name = names.get(id)
      if (name) titles[id] = name
    }
    const body = JSON.stringify({
      id: String(process.pid),
      cwd: process.cwd(),
      status,
      titles,
      blocking: [...pending],
    })
    try {
      const child = spawn(BERTH_BIN, ["notify", "--provider", "pi"], {
        stdio: ["pipe", "ignore", "ignore"],
        windowsHide: true,
      })
      child.on("error", () => {})
      child.stdin.on("error", () => {})
      child.stdin.end(body)
    } catch {}
    inflight = false
  }

  function start() {
    if (heartbeat) return
    heartbeat = setInterval(flush, 1000)
    heartbeat.unref?.()
  }

  function stop() {
    if (heartbeat) clearInterval(heartbeat)
    heartbeat = undefined
  }

  pi.on("session_start", async (event, ctx) => {
    const id = sid(ctx)
    if (event.reason === "new" || event.reason === "resume" || event.reason === "fork") {
      prune(id)
    }
    if (!ctx.isIdle()) {
      seen.add(id)
      running.add(id)
    }
    remember(ctx)
    start()
    flush()
  })

  pi.on("before_agent_start", async (event, ctx) => {
    remember(ctx, event.prompt)
  })

  pi.on("agent_start", async (_event, ctx) => {
    const id = sid(ctx)
    seen.add(id)
    running.add(id)
    remember(ctx)
    flush()
  })

  pi.on("agent_settled", async (_event, ctx) => {
    const id = sid(ctx)
    running.delete(id)
    if (seen.has(id)) prune(id)
    remember(ctx)
    flush()
  })

  pi.on("ui_prompt_start", async (_event, ctx) => {
    const id = sid(ctx)
    seen.add(id)
    pending.add(id)
    remember(ctx)
    flush()
  })

  pi.on("ui_prompt_end", async (_event, ctx) => {
    pending.delete(sid(ctx))
    flush()
  })

  pi.on("session_shutdown", async () => {
    stop()
  })

  pi.on("session_info_changed", async (_event, ctx) => {
    const id = sid(ctx)
    const name = pi.getSessionName?.()
    if (typeof name === "string" && name) names.set(id, name)
    else names.delete(id)
    remember(ctx)
    flush()
  })
}
