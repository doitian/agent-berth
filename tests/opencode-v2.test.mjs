import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { setImmediate } from "node:timers/promises"
import { test } from "node:test"

const source = readFileSync(
  new URL("../src/providers/hooks/opencode-v2/index.js", import.meta.url),
  "utf8"
)
  .replace('import { spawn } from "node:child_process"', "")
  .replace("__AGENT_BERTH_BIN__", '"agent-berth"')
  .replace("export default", "return")

async function reportsFor(events) {
  let flush
  const reports = []
  const spawn = () => ({
    on() {},
    stdin: {
      on() {},
      end(body) { reports.push(JSON.parse(body)) },
    },
  })
  const plugin = new Function("spawn", "setInterval", "clearInterval", `"use strict";\n${source}`)(
    spawn,
    (callback) => { flush = callback; return { unref() {} } },
    () => {}
  )
  const cleanup = await plugin.setup({
    location: { directory: "/work" },
    event: { subscribe: async function* () { yield* events } },
  })
  try {
    await setImmediate()
    flush()
    flush()
    assert.equal(reports.length, 2)
    return reports
  } finally {
    cleanup()
  }
}

const sessionID = "ses_interrupted"
const started = { type: "session.execution.started", data: { sessionID } }

test("a child restarted after interruption retires when it finishes", async () => {
  const events = [
    { type: "session.created", data: { info: { id: sessionID, parentID: "parent" } } },
    started,
    { type: "session.execution.interrupted", data: { sessionID, reason: "superseded" } },
    started,
    { type: "session.execution.succeeded", data: { sessionID } },
  ]
  for (const report of await reportsFor(events)) {
    assert.deepEqual(report.status, {})
  }
})

test("deleting a session forgets its child classification", async () => {
  const events = [
    { type: "session.created", data: { info: { id: sessionID, parentID: "parent" } } },
    { type: "session.deleted", data: { sessionID } },
    { type: "session.created", data: { info: { id: sessionID } } },
    started,
    { type: "session.execution.succeeded", data: { sessionID } },
  ]
  for (const report of await reportsFor(events)) {
    assert.deepEqual(report.status, { [sessionID]: "idle" })
  }
})

test("an unfinished execution stays busy", async () => {
  for (const report of await reportsFor([started])) {
    assert.equal(report.status[sessionID], "busy")
  }
})

for (const terminal of [
  { type: "session.execution.succeeded", data: { sessionID } },
  { type: "session.execution.failed", data: { sessionID } },
  ...["user", "shutdown", "superseded", "inactivity"].map((reason) => ({
    type: "session.execution.interrupted", data: { sessionID, reason },
  })),
]) {
  const label = terminal.data.reason || terminal.type
  for (const child of [false, true]) {
    test(`${label} stops ${child ? "child" : "root"} session heartbeats reporting busy`, async () => {
      const events = [
        { type: "session.execution.started", data: { sessionID: "ses_other" } },
        {
          type: "session.created",
          data: { info: { id: sessionID, ...(child ? { parentID: "parent" } : {}) } },
        },
        started,
        terminal,
      ]
      for (const report of await reportsFor(events)) {
        assert.deepEqual(report.status, {
          ses_other: "busy",
          ...(child ? {} : { [sessionID]: "idle" }),
        })
      }
    })
  }
}
