---
name: agent-berth
description: Start, monitor, and message coding agents (Claude Code, Codex, Grok, OpenCode, Pi) running in tmux through the agent-berth CLI. Use when delegating a task to another agent, checking whether a delegated agent has finished, reading its output, or sending it follow-up instructions.
---

# Managing agents with agent-berth

agent-berth tracks coding agents through hooks and runs them in tmux. A
delegated agent is identified by the **tmux pane id** that `agent-berth new`
prints, and by the stable session **id** that agent-berth assigns once the
agent reports.

Requirements: `agent-berth` and `tmux` on PATH, and the agent-berth server
running with hooks installed (`agent-berth doctor` checks both; fix problems
with `agent-berth setup`). The examples use `jq` to read JSON.

## 1. Start an agent

```sh
pane=$(agent-berth new claude "Fix the failing test in src/parser.rs and run cargo test")
```

- `agent-berth new <command> [args...]` opens a new window in the tmux session
  for the current directory and runs the command there. Every argument after
  `new` is passed to the agent unchanged, flags included.
- stdout is only the pane id (such as `%12`); the human-readable "started"
  line goes to stderr. Always capture stdout.
- Run it from the directory the agent should work in.
- Give the task as the agent's initial prompt argument, and keep the agent
  interactive: never use one-shot modes such as `claude -p` or `codex exec`,
  whose pane closes when they finish.
  - Claude Code: `claude "task"`; Codex: `codex "task"`. For other agents,
    check `<agent> --help` for how to pass an initial prompt.
  - Codex and Grok only appear in agent-berth after their first prompt is
    submitted, so starting them without a prompt leaves nothing to query or
    message. Claude Code can appear earlier, as `idle`, before its first turn
    starts.
  - OpenCode and Pi report through a plugin once they have a session. If one
    shows up without a prompt, give it the task with `agent-berth send`
    (step 4).

## 2. Find the agent and its id

Wait for the agent's first report, but stop when the pane closes (the
command failed to start) or after about two minutes:

```sh
id=
for _ in $(seq 60); do
  id=$(agent-berth search --tmux-pane "$pane" --json | jq -r '.[0].id // empty')
  [ -n "$id" ] && break
  tmux list-panes -a -F '#{pane_id}' | grep -qxF "$pane" || break
  sleep 2
done
[ -n "$id" ] || echo "the agent in $pane never reported" >&2
```

If no id arrives, look at the pane (`tmux capture-pane -p -t "$pane"`) and
run `agent-berth doctor` before retrying.

`search --tmux-pane '<pane-id>' --json` returns the session that pane shows
right now, newest first. It is empty until the agent's first report arrives
(usually a few seconds), and again after the agent exits. It normally holds
one session; OpenCode 1 and Pi do not report which of their sessions is in
front, so one of those processes can match with every session it has opened.
`.[0]` is then the newest one. Useful fields:

| Field | Meaning |
| --- | --- |
| `id` | Stable session id for `logs`, `send`, and `search <id>` |
| `status` | `running`, `waiting`, `done`, or `idle` (see below) |
| `provider` | `claude`, `codex`, `grok`, `opencode`, or `pi` |
| `title` | Session title reported by the agent |
| `cwd` | Working directory |
| `tmux_pane` | `id`, `session`, `window`, `window_name`, `path`, and `pid` of the pane |
| `last_report_ms` | Time of the latest report, in Unix milliseconds |

## 3. Check whether the agent has finished

| `status` | Meaning | What to do |
| --- | --- | --- |
| `running` | Working on the task | Wait and poll again |
| `waiting` | Blocked on a permission prompt or a question | Run `agent-berth logs "$id" --final` and tell the user what it is blocked on; they can answer it with `agent-berth attach "$id"` |
| `done` | Finished its turn (Claude Code, Codex, Grok) | Read the reply with `logs --final` |
| `idle` | Finished its turn (OpenCode, Pi), or has not started one yet | Read the reply with `logs --final` |

`send` cannot answer a `waiting` agent: it types or queues a new message, and
cannot pick an option in a permission dialog.

The status can still read `done` or `idle` for a moment after `new` or `send`,
before the agent's new turn reports. So treat the turn as finished only after
it has been `running`; if it never shows `running` within about two minutes,
stop polling and read the logs. Poll at a modest interval, and do not name the
variable `status`: it is read-only in zsh.

```sh
seen= quiet=0
while :; do
  state=$(agent-berth search --tmux-pane "$pane" --json | jq -r '.[0].status // "gone"')
  case $state in
    running) seen=1 ;;
    waiting|gone) break ;;
    *) { [ -n "$seen" ] || [ $((quiet += 1)) -ge 8 ]; } && break ;;
  esac
  sleep 15
done
```

`gone` means the pane no longer shows the agent: it exited, or the pane was
closed. Look the session up by id instead, which also finds ended sessions:
`agent-berth search "$id" --json`.

A finished turn is not proof the task succeeded. Always read the output
before reporting back.

## 4. Read output and send messages

Transcripts are long, and most of their size is tool calls and results.
Read only what you need, starting with the reply:

```sh
agent-berth logs "$id" --final                  # the agent's latest reply only
agent-berth logs "$id" --turns 1 --tools brief  # the latest turn, one line per tool call
agent-berth logs "$id" --turns 3 --tools none   # the last three requests and replies
agent-berth logs "$id"                          # everything, oldest first

agent-berth send "$id" "Also update the changelog, then stop."
printf 'Line one\nLine two\n' | agent-berth send "$id"   # message from stdin
```

- `--final` prints the agent's last text in its newest turn. Each `send`
  starts a new turn, so it always answers the latest message. If the agent
  has not replied yet, stdout is empty and a note on stderr says so.
- For a `waiting` agent, `--final` ends with `Pending: <tool> <input>` lines
  naming the command it needs permission for or the question it asks
  (verified for Claude Code).
- To check how the agent got there (did it run the tests, which files did it
  change), use `--turns 1 --tools brief`. Raise the count for more history.
- `--json` prints the selected turns; `jq -r '.turns[-1].final'` extracts the
  reply, and `"structured": false` marks a transcript without turns.
- `logs` reads the agent's own session file or export. Grok sessions, and
  sessions whose log cannot be read, fall back to the tmux pane's scrollback,
  which has no turns: `--final` and `--turns` then print its last 40 lines
  (or `--tail N`) and say so on stderr.
- `send` delivers through the agent's own CLI when it has one (Codex,
  OpenCode), otherwise it types the message into the agent's pane. Typing
  needs the agent to be the active session in its pane, and the pane must not
  be in tmux copy mode.
- After `send`, wait for the new turn with the loop from step 3.

## Example: delegate and wait

```sh
pane=$(agent-berth new codex "Add input validation to src/api/users.rs with tests")

id=
for _ in $(seq 60); do
  id=$(agent-berth search --tmux-pane "$pane" --json | jq -r '.[0].id // empty')
  [ -n "$id" ] && break
  tmux list-panes -a -F '#{pane_id}' | grep -qxF "$pane" || break
  sleep 2
done

seen= quiet=0
while [ -n "$id" ]; do
  state=$(agent-berth search --tmux-pane "$pane" --json | jq -r '.[0].status // "gone"')
  case $state in
    running) seen=1 ;;
    waiting|gone) break ;;
    *) { [ -n "$seen" ] || [ $((quiet += 1)) -ge 8 ]; } && break ;;
  esac
  sleep 15
done

[ -n "$id" ] && agent-berth logs "$id" --final
```

## Troubleshooting

- `new` fails with "tmux is required": install tmux, or start the agent
  manually.
- The search stays empty: run `agent-berth doctor`. Hooks may be missing for
  that agent, the server may be stopped, or (for Codex and Grok) no prompt has
  been submitted yet.
- `send` reports "is not the active session in a tmux pane": another tab is in
  front in that pane, or the pane is gone. Check with
  `agent-berth search "$id" --json`.
