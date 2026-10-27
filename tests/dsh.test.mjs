import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"

// The plugin is a Harness module, so it is evaluated here against a stub
// context: the same file `agent-berth setup` writes, with the binary path and
// the module syntax the Cordis loader supplies.
const source =
  readFileSync(new URL("../src/providers/hooks/dsh.mjs", import.meta.url), "utf8")
    .replace(/^import .*$/gm, "")
    .replace("__AGENT_BERTH_BIN__", '"agent-berth"')
    .replace("import.meta.url", '"file:///dsh/agent-berth/plugin.mjs"')
    .replace("export const name =", "const name =")
    .replace("export function apply(ctx)", "function apply(ctx)") +
  "\nreturn { name, apply }\n"

const session = { id: "session-abc", header: { cwd: "/work" } }

function boot({
  sessions = [],
  statuses = {},
  titles = {},
  archived = [],
  registry = true,
  store,
  desktop = false,
} = {}) {
  const reports = []
  const listeners = new Map()
  const timers = []
  // The sessions service drops a session when it is disposed; the workspace
  // registry keeps archived sessions open and only lists them as archived. A
  // profile without the registry reads the same set from the storage domain.
  const open = [...sessions]
  const archive = [...archived]
  const stored = store ?? { global: { archivedSessionIds: archive } }
  let heartbeat
  const spawn = () => ({
    on() {},
    stdin: {
      on() {},
      end(body) {
        reports.push(JSON.parse(body))
      },
    },
  })
  const plugin = new Function(
    "spawn",
    "readFileSync",
    "setInterval",
    "clearInterval",
    "setTimeout",
    "clearTimeout",
    "process",
    `"use strict";\n${source}`,
  )(
    spawn,
    (url) => {
      if (store === null) throw Object.assign(new Error("no archive set"), { code: "ENOENT" })
      if (String(url).replace(/\\/g, "/").endsWith("storages/workspace.json") === false) {
        throw Object.assign(new Error("unexpected read"), { code: "ENOENT" })
      }
      return JSON.stringify(stored)
    },
    (callback) => {
      heartbeat = callback
      return { unref() {} }
    },
    () => {},
    (callback) => {
      timers.push(callback)
      return { unref() {} }
    },
    () => {},
    { pid: 4242, type: desktop ? "browser" : undefined, env: {} },
  )
  plugin.apply({
    on(name, listener) {
      if (!listeners.has(name)) listeners.set(name, [])
      listeners.get(name).push(listener)
    },
    effect(effect) {
      effect()
    },
    get(name) {
      if (name === "sessions") return { list: () => open }
      if (name === "agents") {
        return { get: (id) => (statuses[id] ? { status: statuses[id] } : undefined) }
      }
      if (name === "sessionProjections") {
        return { stateOf: (target) => ({ title: titles[target.id] }) }
      }
      if (name === "workspaceRegistry" && registry) {
        return { archivedSessionIds: archive }
      }
      return undefined
    },
  })
  const pending = () => {
    while (timers.length) timers.shift()()
  }
  return {
    reports,
    archive: (target = session) => {
      stored.global.archivedSessionIds.push(target.id)
    },
    unarchive: (target = session) => {
      const ids = stored.global.archivedSessionIds
      ids.splice(ids.indexOf(target.id), 1)
    },
    emit(type, target = session, data = {}) {
      for (const listener of listeners.get("session/event") ?? []) {
        listener(target, { type, data })
      }
      pending()
    },
    dispose(target = session) {
      open.splice(open.indexOf(target), 1)
      for (const listener of listeners.get("session/disposed") ?? []) listener(target)
      pending()
    },
    heartbeat() {
      heartbeat()
      pending()
    },
    beat() {
      heartbeat()
    },
  }
}

test("a turn reports busy and then idle", () => {
  const app = boot()
  app.emit("turn/start", session, { turn: 1 })
  assert.deepEqual(app.reports.at(-1).status, { [session.id]: "busy" })
  app.emit("turn/end", session, { turn: 1 })
  assert.deepEqual(app.reports.at(-1).status, { [session.id]: "idle" })
})

test("an approval blocks the session until it is answered", () => {
  const app = boot()
  app.emit("turn/start", session, { turn: 1 })
  app.emit("approval/asked", session, { id: "req-1" })
  assert.deepEqual(app.reports.at(-1).blocking, [session.id])
  app.emit("approval/decided", session, { id: "req-1" })
  assert.deepEqual(app.reports.at(-1).blocking, [])
})

test("the turn end clears an approval left unanswered", () => {
  const app = boot()
  app.emit("turn/start", session, { turn: 1 })
  app.emit("approval/asked", session, { id: "req-1" })
  app.emit("turn/end", session, { turn: 1 })
  assert.deepEqual(app.reports.at(-1).blocking, [])
})

test("a title event is forwarded and the working directory follows the session", () => {
  const app = boot()
  app.emit("session/title", session, { title: "Fix the parser" })
  assert.equal(app.reports.at(-1).titles[session.id], "Fix the parser")
  assert.equal(app.reports.at(-1).cwd, "/work")
})

test("sessions open before the plugin loaded are reported on the first heartbeat", () => {
  const app = boot({ sessions: [session], statuses: { [session.id]: "running" } })
  app.heartbeat()
  assert.deepEqual(app.reports.at(-1).status, { [session.id]: "busy" })
  assert.equal(app.reports.at(-1).cwd, "/work")
})

test("a session open before the plugin loaded keeps its title", () => {
  const app = boot({
    sessions: [session],
    titles: { [session.id]: "Fix the parser" },
  })
  app.heartbeat()
  assert.equal(app.reports.at(-1).titles[session.id], "Fix the parser")
})

test("a session without a title reports none", () => {
  const app = boot({ sessions: [session] })
  app.heartbeat()
  assert.deepEqual(app.reports.at(-1).titles, {})
})

test("only the desktop app's sessions say they are desktop sessions", () => {
  const app = boot({ sessions: [session], desktop: true })
  app.heartbeat()
  assert.equal(app.reports.at(-1).source, "desktop")

  const cli = boot({ sessions: [session] })
  cli.heartbeat()
  assert.equal(cli.reports.at(-1).source, undefined)
})

test("events that do not change status report nothing", () => {
  const app = boot({ sessions: [session] })
  app.heartbeat()
  const before = app.reports.length
  app.emit("tool/call", session, { id: "call-1" })
  assert.equal(app.reports.length, before)
})

test("a disposed session's row is cleared and then stops being reported", () => {
  const app = boot({ sessions: [session] })
  app.heartbeat()
  app.dispose(session)
  assert.deepEqual(app.reports.at(-1).status, {})
  assert.equal(app.reports.at(-1).id, session.id)
  const cleared = app.reports.length
  app.heartbeat()
  assert.equal(app.reports.length, cleared)
})

test("an archived session open at startup gets one clear and nothing else", () => {
  const app = boot({ sessions: [session], archived: [session.id] })
  app.heartbeat()
  assert.deepEqual(
    app.reports.map((report) => report.status),
    [{}],
  )
  const cleared = app.reports.length
  app.heartbeat()
  assert.equal(app.reports.length, cleared)
})

test("archiving clears the row instead of leaving it idle", () => {
  const app = boot({ sessions: [session] })
  app.heartbeat()
  assert.deepEqual(app.reports.at(-1).status, { [session.id]: "idle" })
  app.archive()
  app.heartbeat()
  assert.deepEqual(app.reports.at(-1).status, {})
  const cleared = app.reports.length
  app.heartbeat()
  assert.equal(app.reports.length, cleared)
})

test("the turn end that archiving stops does not bring the row back", () => {
  const app = boot({ sessions: [session] })
  app.emit("turn/start", session, { turn: 1 })
  app.archive()
  const before = app.reports.length
  app.emit("turn/end", session, { turn: 1 })
  assert.equal(app.reports.length, before)
})

test("unarchiving reports the session again", () => {
  const app = boot({ sessions: [session], archived: [session.id] })
  app.heartbeat()
  app.unarchive()
  app.heartbeat()
  assert.deepEqual(app.reports.at(-1).status, { [session.id]: "idle" })
})

test("a terminal profile reads the archive set the app writes", () => {
  const app = boot({ sessions: [session], registry: false, archived: [session.id] })
  app.heartbeat()
  assert.deepEqual(
    app.reports.map((report) => report.status),
    [{}],
  )
})

test("archiving from the app clears a terminal session's row", () => {
  const app = boot({ sessions: [session], registry: false })
  app.heartbeat()
  assert.deepEqual(app.reports.at(-1).status, { [session.id]: "idle" })
  app.archive()
  app.heartbeat()
  assert.deepEqual(app.reports.at(-1).status, {})
  const cleared = app.reports.length
  app.heartbeat()
  assert.equal(app.reports.length, cleared)
})

test("an unreadable archive set hides nothing", () => {
  const app = boot({ sessions: [session], registry: false, store: null })
  app.heartbeat()
  assert.deepEqual(app.reports.at(-1).status, { [session.id]: "idle" })
})

test("rows an earlier process left for archived sessions are cleared too", () => {
  const app = boot({ archived: ["session-old-1", "session-old-2"] })
  app.heartbeat()
  assert.deepEqual(
    app.reports.map((report) => report.id),
    ["session-old-1", "session-old-2"],
  )
  const cleared = app.reports.length
  app.heartbeat()
  assert.equal(app.reports.length, cleared)
})

test("a long archive set drains a batch per flush", () => {
  const ids = Array.from({ length: 40 }, (_, index) => `session-${index}`)
  const app = boot({ archived: ids })
  app.beat()
  assert.equal(app.reports.length, 32)
  app.beat()
  assert.equal(app.reports.length, 40)
})
