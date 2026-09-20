import { spawn } from "node:child_process"

const BERTH_BIN = __AGENT_BERTH_BIN__

export default () => {
  const seen = new Set()
  const running = new Set()
  const child = new Set()
  const pending = new Map()
  const names = new Map()
  let inflight = false

  function drop(sid) {
    seen.delete(sid)
    running.delete(sid)
    child.delete(sid)
    names.delete(sid)
    for (const [id, session] of pending) if (session === sid) pending.delete(id)
  }

  function prune(keep) {
    const blocked = new Set(pending.values())
    for (const sid of [...seen]) {
      if (sid === keep || running.has(sid) || blocked.has(sid)) continue
      drop(sid)
    }
  }

  function flush() {
    if (inflight) return
    inflight = true
    const status = {}
    for (const sid of seen) status[sid] = running.has(sid) ? "busy" : "idle"
    const titles = {}
    for (const sid of seen) {
      const name = names.get(sid)
      if (name) titles[sid] = name
    }
    const body = JSON.stringify({
      id: String(process.pid),
      cwd: process.cwd(),
      status,
      titles,
      blocking: [...new Set(pending.values())],
    })
    try {
      const childProc = spawn(BERTH_BIN, ["notify", "--provider", "opencode"], {
        stdio: ["pipe", "ignore", "ignore"],
        windowsHide: true,
      })
      childProc.on("error", () => {})
      childProc.stdin.on("error", () => {})
      childProc.stdin.end(body)
    } catch {}
    inflight = false
  }

  function apply(event) {
    const type = String(event?.type || "")
    const props = event?.properties || {}
    const sid = props.sessionID || props.info?.id
    const requestID = props.id || props.permissionID || props.requestID
    if (typeof props.info?.title === "string" && props.info.id) {
      names.set(props.info.id, props.info.title)
    }
    if (type === "session.created" && sid) {
      if (props.info?.parentID) child.add(sid)
      else prune(sid)
    } else if (type === "tui.session.select" && sid) {
      prune(sid)
    } else if (type === "tui.command.execute" && props.command === "session.new") {
      prune()
    } else if (type === "session.status" && sid) {
      const kind = props.status?.type
      if (kind === "busy" || kind === "retry") {
        seen.add(sid)
        running.add(sid)
      } else {
        running.delete(sid)
        if (child.has(sid)) drop(sid)
        else if (seen.has(sid)) prune(sid)
      }
    } else if (type === "session.idle" && sid) {
      running.delete(sid)
      if (child.has(sid)) drop(sid)
      else if (seen.has(sid)) prune(sid)
    } else if (type === "session.deleted" && sid) {
      drop(sid)
    } else if (sid && requestID && /permission|question/.test(type) && /asked|updated/.test(type)) {
      seen.add(sid)
      pending.set(requestID, sid)
    } else if (requestID && /replied|rejected/.test(type)) {
      pending.delete(requestID)
    }
  }

  const heartbeat = setInterval(flush, 1000)
  heartbeat.unref?.()
  return {
    event: ({ event }) => apply(event),
  }
}
