import { spawn } from "node:child_process"

const BERTH_BIN = __AGENT_BERTH_BIN__

function fields(event) {
  const data = event?.data
  if (data && typeof data === "object") return data
  const props = event?.properties
  if (props && typeof props === "object") return props
  return {}
}

function eventDirectory(event) {
  const location = event?.location
  if (!location) return ""
  if (typeof location === "string") return location
  return location.directory || ""
}

function sameDir(left, right) {
  const fold = (value) => {
    let text = String(value).replace(/\\/g, "/").replace(/\/+$/, "")
    if (process.platform === "win32") text = text.toLowerCase()
    return text
  }
  return fold(left) === fold(right)
}

export default {
  id: "agent-berth-v2",
  async setup(ctx) {
    const seen = new Set()
    const running = new Set()
    const child = new Set()
    const pending = new Map()
    const names = new Map()
    const sessionDirs = new Map()
    const locating = new Set()
    const queued = []
    let inflight = false
    const cwd = ctx?.location?.directory || process.cwd()
    // The server loads one plugin instance per location; keep their reports
    // apart so a home-location instance cannot overwrite this one.
    const instance = `${process.pid}:${cwd}`

    function drop(sid) {
      seen.delete(sid)
      running.delete(sid)
      child.delete(sid)
      names.delete(sid)
      sessionDirs.delete(sid)
      for (const [id, session] of pending) if (session === sid) pending.delete(id)
    }

    // Sessions created before this plugin loaded never announce their
    // location, so look it up once and gate their events on the result.
    function locate(sid) {
      if (sessionDirs.has(sid) || !ctx?.session?.get) {
        return true
      }
      if (locating.has(sid)) {
        return false
      }
      locating.add(sid)
      const replay = () => {
        locating.delete(sid)
        for (const event of queued.splice(0)) apply(event)
      }
      try {
        Promise.resolve(ctx.session.get({ sessionID: sid }))
          .then((result) => {
            const info = result?.data ?? result
            const dir = info?.location?.directory || ""
            sessionDirs.set(sid, dir)
            if (typeof info?.title === "string" && info.title) {
              names.set(sid, info.title)
            }
          })
          .catch(() => sessionDirs.set(sid, ""))
          .finally(replay)
      } catch {
        sessionDirs.set(sid, "")
        replay()
      }
      return false
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
        id: instance,
        pid: process.pid,
        cwd,
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
      const directory = ctx?.location?.directory
      const eventDir = eventDirectory(event)
      const type = String(event?.type || "")
      const props = fields(event)
      const sid = props.sessionID || props.info?.id
      if (type === "session.deleted" && sid) {
        drop(sid)
        return
      }
      // Execution and streaming events carry no location; remember where
      // each session was created so this instance only tracks its own
      // location's sessions even without a location on the event.
      if (type === "session.created" && sid && eventDir) {
        sessionDirs.set(sid, eventDir)
      }
      let scope = eventDir
      if (!scope && sid && type.startsWith("session.")) {
        if (!locate(sid)) {
          queued.push(event)
          return
        }
        scope = sessionDirs.get(sid)
      }
      if (directory && scope && !sameDir(directory, scope)) return
      const requestID = props.id || props.permissionID || props.requestID
      // Any event for a session of this location proves it is live here;
      // execution started events may predate this plugin load, so streaming
      // and tool events also keep the session tracked and running.
      if (sid && type.startsWith("session.")) {
        seen.add(sid)
        if (
          /^(session\.(step\.|text\.|reasoning\.|tool\.)|session\.inbox\.(enqueued|delivered))/.test(
            type
          )
        ) {
          running.add(sid)
        }
      }
      if (typeof props.info?.title === "string" && props.info.id) {
        names.set(props.info.id, props.info.title)
      }
      if (type === "session.renamed" && sid && typeof props.title === "string") {
        names.set(sid, props.title)
      } else if (type === "session.created" && sid) {
        // Sessions coexist as tabs; a new one does not retire the rest.
        // Closed tabs stop emitting events, so the berth hides them through
        // the tab list its TUI part reports.
        seen.add(sid)
        if (props.info?.parentID || props.parentID) child.add(sid)
      } else if (sid && (type === "session.execution.started" || type === "session.retry.scheduled")) {
        seen.add(sid)
        running.add(sid)
      } else if (sid && (type === "session.execution.succeeded" || type === "session.execution.failed")) {
        running.delete(sid)
        if (child.has(sid)) drop(sid)
      } else if (sid && requestID && /permission|question/.test(type) && /asked|updated/.test(type)) {
        seen.add(sid)
        pending.set(requestID, sid)
      } else if (requestID && /replied|rejected/.test(type)) {
        pending.delete(requestID)
      }
    }

    const controller = new AbortController()
    const heartbeat = setInterval(flush, 1000)
    heartbeat.unref?.()
    if (typeof ctx?.event?.subscribe === "function") {
      try {
        let subscription = ctx.event.subscribe({ signal: controller.signal })
        if (subscription && typeof subscription.then === "function") {
          subscription = await subscription
        }
        const watcher = (async () => {
          for await (const event of subscription) apply(event)
        })()
        watcher.catch(() => {})
      } catch {}
    }
    return () => {
      controller.abort()
      clearInterval(heartbeat)
    }
  },
}
