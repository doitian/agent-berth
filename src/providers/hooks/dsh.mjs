import { spawn } from "node:child_process"
import { readFileSync } from "node:fs"

const BERTH_BIN = __AGENT_BERTH_BIN__

// The json storage domain holding the archive set, beside this plugin's
// directory in `$DSH_HOME`.
const STORE = new URL("../storages/workspace.json", import.meta.url)

// The last archive set read from the store, so a torn read keeps it.
let lastStored = new Set()

// Written by `agent-berth setup` into $DSH_HOME and inserted by a row in
// $DSH_HOME/cordis.patch.yml, so every booted profile reports its sessions.
// Each session is reported under its own id, because sessions of one app can
// live in different working directories and a snapshot carries one cwd.

export const name = "agent-berth"

const HEARTBEAT_MS = 2000
const DEBOUNCE_MS = 100
// How many archived ids one flush asks berth to drop.
const CLEAR_BATCH = 32

// The events that change what berth shows; a session/event for anything else
// is not worth reading the archive set for.
const REPORTED = new Set([
  "turn/start",
  "turn/end",
  "approval/asked",
  "approval/decided",
  "session/title",
])

export function apply(ctx) {
  // session id -> { busy, blocked: Set<request id>, title, cwd }
  const sessions = new Map()
  // Archived ids whose idle row was already cleared, so the heartbeat reports
  // the clear once instead of on every tick. A session that is reported again,
  // including an archived one that is running or waiting, leaves the set.
  const cleared = new Set()
  // The app boots its profile in the Electron main process, which is what
  // separates a desktop session from one a terminal booted.
  const desktop = process.type === "browser" || process.env.DSH_PROFILE === "desktop"
  let debounce
  let heartbeat

  function entry(id, session) {
    let state = sessions.get(id)
    if (state === undefined) {
      state = { busy: false, blocked: new Set(), title: title(session), cwd: cwd(session) }
      sessions.set(id, state)
    } else if (!state.cwd) {
      state.cwd = cwd(session)
    }
    return state
  }

  function cwd(session) {
    const cwd = session?.header?.cwd
    return typeof cwd === "string" ? cwd : ""
  }

  // A session that was already open when the plugin loaded has been titled
  // before its first `session/title` event. The projection is optional, so a
  // profile without the title plugin seeds nothing.
  function title(session) {
    try {
      const state = ctx.get?.("sessionProjections")?.stateOf?.(session, "title")
      return typeof state?.title === "string" ? state.title : ""
    } catch {
      return ""
    }
  }

  function schedule() {
    if (debounce !== undefined) return
    debounce = setTimeout(() => {
      debounce = undefined
      flush()
    }, DEBOUNCE_MS)
    debounce.unref?.()
  }

  // Archived sessions stay open and keep their place in the session store — the
  // archive set is durable workspace state, not a session lifecycle event — so
  // nothing disposes them. Read it on each flush to mirror what the app shows.
  // Only the profiles with the workspace UI load that registry, so a terminal
  // profile reads the same set from the storage domain every profile shares.
  function archive() {
    let ids
    try {
      ids = ctx.get?.("workspaceRegistry")?.archivedSessionIds
    } catch {
      ids = undefined
    }
    return new Set(Array.isArray(ids) ? ids : stored())
  }

  function stored() {
    try {
      const state = JSON.parse(readFileSync(STORE, "utf8"))
      const ids = Array.isArray(state?.global?.archivedSessionIds)
        ? state.global.archivedSessionIds
        : []
      lastStored = new Set(ids)
      return ids
    } catch {
      // A write from the app can be caught mid-flight; keep the last set read
      // whole instead of flashing every archived session back for a heartbeat.
      return [...lastStored]
    }
  }

  function live(state) {
    return !!state && (state.busy || state.blocked.size > 0)
  }

  // Sessions that were already live when the plugin loaded, and ones that have
  // not reported an event since, still need a first report. An archived
  // session is one of them only while it is running or waiting.
  function sync(hidden) {
    for (const session of ctx.get?.("sessions")?.list?.() ?? []) {
      const id = session.id
      const running = ctx.get?.("agents")?.get?.(id)?.status === "running"
      const state = sessions.get(id)
      if (hidden.has(id) && !running && !(state?.blocked.size > 0)) continue
      cleared.delete(id)
      if (state !== undefined) {
        if (running) state.busy = true
        continue
      }
      const created = entry(id, session)
      created.busy = running
    }
  }

  function flush() {
    const hidden = archive()
    // An archived session that has gone idle is not reported; the row it had
    // is cleared by `clearRows` below. One that is running or waiting stays.
    for (const id of [...sessions.keys()]) {
      if (hidden.has(id) && !live(sessions.get(id))) sessions.delete(id)
    }
    sync(hidden)
    for (const [id, state] of sessions) report(id, state)
    clearRows(hidden)
  }

  // The archive set outlives any one process, so it drains a batch per flush
  // rather than asking berth to drop everything ever archived at once. A clear
  // is what removes the row a reporting pid would otherwise hold past the
  // staleness window. A session currently reported as running or waiting is
  // not cleared.
  function clearRows(hidden) {
    const queue = [...hidden].filter((id) => !cleared.has(id) && !sessions.has(id))
    for (const id of queue.slice(0, CLEAR_BATCH)) {
      cleared.add(id)
      report(id)
    }
  }

  // Without a state the report clears the session, which is how an archived
  // session's row goes away.
  function report(id, state) {
    const body = JSON.stringify({
      id,
      pid: process.pid,
      ...(state === undefined
        ? { status: {} }
        : {
            cwd: state.cwd,
            status: { [id]: state.busy ? "busy" : "idle" },
            titles: state.title ? { [id]: state.title } : {},
            blocking: state.blocked.size > 0 ? [id] : [],
          }),
      ...(desktop ? { source: "desktop" } : {}),
    })
    try {
      const child = spawn(BERTH_BIN, ["notify", "--provider", "dsh"], {
        stdio: ["pipe", "ignore", "ignore"],
        windowsHide: true,
      })
      child.on("error", () => {})
      child.stdin.on("error", () => {})
      child.stdin.end(body)
    } catch {}
  }

  ctx.on("session/event", (session, event) => {
    const id = session?.id
    if (typeof id !== "string" || !id) return
    if (!REPORTED.has(event?.type)) return
    const data = event?.data ?? {}
    const state = entry(id, session)
    switch (event?.type) {
      case "turn/start":
        state.busy = true
        break
      case "turn/end":
        state.busy = false
        state.blocked.clear()
        break
      case "approval/asked":
        // A permission prompt or a question is waiting on the user.
        state.blocked.add(String(data.id ?? ""))
        break
      case "approval/decided":
        state.blocked.delete(String(data.id ?? ""))
        break
      case "session/title":
        if (typeof data.title === "string" && data.title) state.title = data.title
        break
      default:
        return
    }
    // An archived session stays out of the list once it is idle. A turn or an
    // approval still shows, and the turn end that archiving stops clears the
    // row instead of leaving it idle.
    if (archive().has(id) && !live(state)) {
      sessions.delete(id)
      if (!cleared.has(id)) {
        cleared.add(id)
        report(id)
      }
      return
    }
    cleared.delete(id)
    schedule()
  })

  // A disposed session's row has to be cleared the same way, because the
  // reporting pid stays alive and holds the snapshot past the staleness window.
  ctx.on("session/disposed", (session) => {
    const id = session?.id
    if (typeof id !== "string" || !id) return
    if (!sessions.delete(id)) return
    report(id)
  })

  heartbeat = setInterval(flush, HEARTBEAT_MS)
  heartbeat.unref?.()
  schedule()

  ctx.effect(() => () => {
    clearInterval(heartbeat)
    if (debounce !== undefined) clearTimeout(debounce)
  }, "agent-berth session reporting")
}
