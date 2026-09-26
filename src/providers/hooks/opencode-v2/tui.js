import { spawn } from "node:child_process"

const BERTH_BIN = __AGENT_BERTH_BIN__

function front(context) {
  const route = context?.ui?.router?.current?.()
  if (route?.type === "session" && route.sessionID) return route.sessionID
  const tabs = context?.ui?.tabs?.enabled?.() ? (context.ui.tabs.list?.() ?? []) : []
  const active = tabs.find((tab) => tab.active)
  return active?.sessionID || null
}

function tabIds(context) {
  const tabs = context?.ui?.tabs?.enabled?.() ? (context.ui.tabs.list?.() ?? []) : []
  const ids = new Set(
    tabs.map((tab) => tab.sessionID).filter((sid) => typeof sid === "string")
  )
  const route = context?.ui?.router?.current?.()
  if (route?.type === "session" && route.sessionID) ids.add(route.sessionID)
  return [...ids]
}

export default {
  id: "agent-berth-v2-tui",
  setup(context) {
    const cwd = context?.location?.directory || process.cwd()

    function flush() {
      const body = JSON.stringify({
        id: String(process.pid),
        pid: process.pid,
        cwd,
        status: {},
        front: front(context),
        tabs: tabIds(context),
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
    }

    const heartbeat = setInterval(flush, 1000)
    heartbeat.unref?.()
    return () => clearInterval(heartbeat)
  },
}
