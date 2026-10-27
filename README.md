# agent-berth

Monitor coding agents and resume their sessions.

agent-berth runs a small background server that tracks the status of your AI
coding agent sessions — Claude Code, CodeBuddy, Codex, DeepSeek Harness, Grok,
OpenCode, and Pi — through hooks installed into each agent. When the server or
the host restarts, it can resume the sessions you were working on, each in its
own tmux window, grouped by working directory.

## Features

- **Live status tracking** — see which sessions are `running`, `waiting`,
  `idle`, or `done`, across all supported agents.
- **Session resume** — restart sessions that were interrupted by a reboot or
  server restart, each in its own tmux window via the agent's native resume
  command (`claude --resume`, `codex resume`, …).
- **Attach** — fuzzy-find a running agent with fzf and jump to its tmux pane
  or focus its session in Paseo.
- **Session search** — find stored sessions across agents by keywords and
  creation time, even when they are no longer active.
- **Persistent state** — sessions survive server and host restarts (redb
  database on disk).
- **Cross-platform** — Linux, macOS, and Windows; IPC over Unix sockets or
  Windows named pipes.

## Install

Prebuilt binaries for Windows, Linux, and macOS are attached to each
[GitHub Release](https://github.com/doitian/agent-berth/releases). With
[cargo-binstall](https://github.com/cargo-bins/cargo-binstall):

```sh
cargo binstall agent-berth
```

Or build from source (Rust 2024 edition toolchain):

```sh
cargo install agent-berth
# or from a checkout:
cargo install --path .
```

## Quick start

```sh
# Install the background service and hooks for every detected agent
agent-berth setup

# Watch your agents
agent-berth list

# After a reboot, bring back sessions idle for less than 20 minutes
agent-berth resume
```

`setup` installs hooks only for agents that are installed (found on `PATH` or
with an existing config directory), and registers the server as a user
service: a systemd user unit on Linux, a Scheduled Task on Windows. Use
`agent-berth setup --no-service` to install hooks only.

Resuming CLI sessions requires [tmux](https://github.com/tmux/tmux) (or
[psmux](https://github.com/psmux/psmux) on Windows). Interactive selection
uses [fzf](https://github.com/junegunn/fzf).

## Commands

| Command | Description |
| --- | --- |
| `agent-berth tui [--tmux]` | Interactive TUI (also the default with no subcommand); `--tmux` attaches to a pane already running the TUI, or opens one in a new window |
| `agent-berth setup [--no-service]` | Install the user service and agent hooks |
| `agent-berth teardown` | Stop the service and remove all agent hooks |
| `agent-berth service start\|stop\|restart` | Manage the background server |
| `agent-berth server` | Run the server in the foreground |
| `agent-berth list [--json] [--resumable [--idle 20m] [--here]]` | List sessions |
| `agent-berth search [patterns...] [--created-in DURATION] [--delegated-by ID] [--tmux-pane PANE_ID] [--json]` | Search stored sessions across all providers, newest first; `--delegated-by` keeps only sessions started by that session, `--tmux-pane` only the agent a tmux pane shows now |
| `agent-berth self [--json]` | Print the stable id of the agent session running the command, from the provider's shell-tool environment when it exposes one, otherwise from the enclosing tmux pane |
| `agent-berth logs <id> [--final \| --turns N] [--tools full\|brief\|none] [--tail N] [--json]` | Print a session's transcript, its latest reply, or its latest turns |
| `agent-berth stats [--json]` | Aggregate active session counts by status and provider |
| `agent-berth new [--wait [--timeout 2m]] [--delegator ID \| --no-delegator] <command> [args...]` | Start an agent in a new window of the current directory's tmux session, the one `resume` uses, creating it when missing, and print the new pane id; for example `agent-berth new claude --model opus`. With `--wait`, print the stable session id once the agent first reports (exit 4 on timeout, exit 1 when the pane closes first). The window gets `AGENT_BERTH_DELEGATOR` set to the caller's id |
| `agent-berth resume [pattern] [--idle 20m] [--here] [--dry-run]` | Resume sessions in tmux |
| `agent-berth attach [query] [--preview] [--session] [--dry-run]` | Select an agent running in a tmux pane with fzf and attach to it; Paseo and desktop sessions are only reachable from the TUI |
| `agent-berth memo [pattern] [text...]` | Set a note for the selected session, shown instead of its detected title and matched by search; no text prints the current note, `--clear` removes it |
| `agent-berth send [--wait [--timeout 10m]] [--force] [--raw] <id> "message"` | Send a message to a session through the agent's CLI, or into its tmux pane; `--wait` then waits for the turn it starts to end, as `wait` does; a `waiting` session is refused without `--force`; `--raw` drops the sender line |
| `agent-berth reply [--wait [--timeout 10m]] [--force] [--raw] "message"` | Send a message to the session that started this agent with `new`, exactly as `send` would |
| `agent-berth wait <id> [--after N] [--timeout 10m]` | Wait for a session to end a turn; prints `done`, `waiting`, `gone`, or `timeout` with exit code 0, 2, 3, or 4 |
| `agent-berth stop <id> [--force] [--dry-run]` | End a session by killing the tmux pane that shows it; refused when the pane also hosts other sessions unless `--force` |
| `agent-berth rm [patterns...]` | Hide sessions until they are running or waiting |
| `agent-berth doctor` | Check server, service, and hook installation |
| `agent-berth notify --provider <name>` | Report agent status (used by hooks) |

### TUI

`agent-berth tui` (or bare `agent-berth`) opens an interactive dashboard.
With `--tmux` it instead attaches to a tmux pane already running the TUI
(switching clients inside tmux, attaching from outside); when no pane runs
it, a new window is created in the current session — or the most recently
used one outside tmux — and attached. The left column lists sessions with the agent logo, status, and title; the
right column shows session details, plus a live tmux pane preview when the
selected session runs in tmux. Local Claude Code sessions in Claude Desktop
and Codex desktop sessions show a live conversation preview instead, with
user messages, assistant responses, and concise tool activity. CLI sessions
fall back to the same conversation preview when no front tmux pane shows
their output — no pane at all, or the pane hosts the session only as a
background tab — if the provider supports transcripts (Claude, CodeBuddy,
Codex, and Pi).

Conversation previews follow the transcript path reported by agent hooks. For
already-running sessions, agent-berth also looks for a matching session file
under `CLAUDE_CONFIG_DIR/projects`, `CODEBUDDY_CONFIG_DIR/projects`,
`CODEX_HOME/sessions`, or `PI_CODING_AGENT_DIR/sessions` (using the default
provider directories when these variables are unset). If no transcript can
be located, the preview waits for a hook to provide its path. Run `setup` to
install hooks, then send a prompt in the session.

Previews refresh about once a second as transcript records reach disk;
they do not stream individual tokens. Switching back to a session shows its
cached preview immediately while fresh content loads. Recent history and long
messages/tool results are bounded to keep the TUI responsive. Missing or unreadable files
are retried automatically. These previews cover local Code sessions, not
ordinary Claude chats or remote/cloud sessions. Claude's optional
`MessageDisplay` hook is not required or installed.

Sessions hosted in [Paseo](https://paseo.sh) show a live conversation stream
through its daemon API, including providers without local transcript support.
Press `a` to focus that session by passing a `paseo://` deep link directly to
the Paseo Desktop executable, without relying on an OS URL handler.
The CLI `attach` command lists only tmux panes and skips Paseo sessions.
No Paseo CLI is required; the local daemon must be running. Focusing
looks for Desktop in standard installation locations and Windows Scoop
(`~/scoop/apps/paseo/current/Paseo.exe`). Set `PASEO_DESKTOP_BIN` to its executable
path for a custom installation. Linux launches `Paseo` or `Paseo.AppImage` with
the URL argument; macOS launches the app bundle using `open --args`.

Paseo identity is forwarded from `PASEO_AGENT_ID` in the hook/plugin environment.
For already-running sessions, agent-berth also matches the native session ID to
local records under `$PASEO_HOME/agents` (default `~/.paseo/agents`). This only
routes sessions already monitored by agent-berth; it does not add sessions or
change their status tracking or resume behavior. A later native host report
clears an older Paseo association, while a newer Paseo import routes the same
session back to Paseo. Host changes, rather than repeated heartbeats, determine
the route; preview and focus switch on refresh. Before focusing or streaming,
agent-berth verifies the session through the API. It reads the daemon endpoint
from `$PASEO_HOME/paseo.pid` rather than assuming a port, supporting loopback TCP,
Unix sockets, and Windows named pipes. Authentication uses
`$PASEO_HOME/local-credential` or `PASEO_PASSWORD` if set. This integration targets
the local daemon in `PASEO_HOME`, not remote `PASEO_HOST` connections. The stream
is bounded, stops when you leave the session or quit, and reconnects after
interruption.

Press `a` on a Claude or Codex desktop session to open it in the app.
Codex uses `codex://threads/<session-id>` to select the session and raise the
desktop app. For Claude on Linux under niri, agent-berth first tries to raise
the Claude window, ignoring errors, then launches the session deep link.
Both launch without waiting for the app to exit.

The details pane combines the Git branch and status in one row, for example
`main [!+↕]`. It uses Starship-style symbols: red `!` for conflicts, `$` for
stashes, `✘` for deletions, yellow `»` for renames, yellow `!` for modifications,
green `+` for staged changes, and `?` for untracked files. Tracking status uses
`↑` (ahead), `↓` (behind), `↕` (diverged), or green `≡` (up to date), relative to
the local upstream tracking ref. Branches without an upstream omit the tracking
symbol. Clean repositories without an upstream show only the branch name.

| Key | Action |
| --- | --- |
| `j`/`k` | Move the selection |
| `/` | Filter sessions by keywords (Enter applies, Esc clears) |
| `ga` / `gr` | Switch to the active / resumable session list |
| `st` | Sort by creation time, newest first (default) |
| `sr` | Sort by activity, most recent first |
| `sa` | Sort by agent provider alphabetically |
| `ss` | Sort by status: waiting, running, done, idle |
| `sd` | Sort by working directory alphabetically, missing directories last |
| `w` | Toggle group headers for the current sort (hidden by default) |
| `I` | In the resumable list, toggle idle sessions; asks for the idle window (default `20m`, prefilled) |
| `=` | Toggle maximization of the tmux or conversation preview |
| `a` | Focus a Paseo, Claude Desktop, or Codex desktop session, or attach to its tmux pane: switch-client inside tmux, attach outside; detaching returns to the TUI |
| `r` | Resume the selected resumable session in tmux and attach to it |
| `d` | Delete the selected session after confirming with `y` |
| `m` | Edit the selected session's memo, shown instead of its detected title; submit an empty memo to clear it |
| `M` | Type a message and send it to the selected active session, like `send` |
| `q` | Quit |

Group headers use `1h`, `1d`, `7d`, and `>7d` for creation/activity sorting:
up to one hour, over one hour through one day, over one day through seven
days, and older than seven days. Other sorts group by agent name, status,
or full working directory. Directory labels show the last two components
with `/` separators, such as `codebase/agent-berth`; missing directories show
`-`. Headers are skipped when moving between sessions.

### Search

Use `search` when you cannot remember which coding CLI you used:

```sh
agent-berth search --created-in 3h
agent-berth search login --created-in 2d
agent-berth search claude my-project --json
```

With no keywords, it shows all stored sessions, including inactive sessions
and closed tabs, newest first. Keywords are case-insensitive; every term must
match the provider, session ID, title, memo, status, working directory, or current
Git branch. `--created-in` restricts creation time, not recent activity, and
accepts seconds, minutes, hours, or days (e.g. `30m`, `1.5h`, `2d`). The table's
`CREATED` column shows the age since creation; JSON includes `created_ms`.

Search works with or without the server. Sessions hidden with `rm` remain
hidden. It searches agent-berth's retained state, not full transcript contents
or every agent's historical archive; ended or pruned sessions may no longer
be stored. The server prunes sessions after seven days without reports unless
their process is still alive.

Every session has a stable short ID, shown in the `ID` column and included as
`id` in `list --json` and `search --json`. The ID is derived from the provider
and native session ID, so it survives restarts and database rebuilds. Commands
that take patterns (`attach`, `resume`, `rm`, `search`, `logs`) also match the ID; for
example, `agent-berth attach <id>` focuses a pane without opening the picker.
An agent prints its own ID with `agent-berth self` (the session's JSON with
`--json`), derived from the provider's shell-tool environment — Claude Code's
`CLAUDE_CODE_SESSION_ID`, CodeBuddy's `CODEBUDDY_SESSION_ID`, or Pi's
`PI_SESSION_ID` — or from the session shown
in `$TMUX_PANE`. A Claude Code subagent shares its parent's environment, so
`self` inside one names the parent session, not the `<session>:<agent>` key
its hooks report.

Sessions running in a tmux pane also carry `tmux_pane` in `list --json` and
`search --json`: the pane `id` (such as `%7`), its tmux `session`, `window`
index, `window_name`, current `path`, and `pid`. The field is omitted for
sessions outside tmux, such as desktop and Paseo sessions or ones whose
process has ended.

`search --tmux-pane <pane-id>` finds the agent a pane shows right now, such as
the pane id printed by `new` or `"$TMUX_PANE"` inside a pane. It matches only
the session the pane displays: an OpenCode 2 TUI hosting several sessions
matches its front tab. OpenCode 1 and Pi do not report which of their
sessions is in front, so the pane matches every session that process has
opened, newest first. A freshly started agent appears once its first hook or
plugin report arrives.

```sh
pane=$(agent-berth new claude)
agent-berth search --tmux-pane "$pane" --json
```

### Transcripts

`agent-berth logs <id>` prints a session's transcript, oldest message first.
It renders the same way as the TUI preview but without the preview's
recent-history or message-length limits, and `--tail N` prints only the last N
lines.

Transcripts are long, and most of their size is tool calls and results, so
`logs` can also print just part of one. It groups the conversation into
turns, each starting with a prompt and holding the agent's work on it:

- `--final` prints only the agent's latest reply: its last text in the newest
  turn, followed by `Pending:` lines for tool calls still waiting on a result,
  such as a command waiting for permission. When the agent has not replied to
  its newest prompt, or is still working, a note on stderr says so and stdout
  holds only the reply.
- `--turns N` prints the newest N turns that have output, each from its
  prompt, leaving out text the agent's harness injected (skill bodies,
  environment details, command output).
- `--tools brief` shortens each tool call and result to one line, and
  `--tools none` drops them except calls still waiting on a result.
- `--json` prints the selected turns, with each turn's `index`, `prompt`,
  `final` reply, `complete` flag, and entries. A transcript read from a tmux
  pane has no turns and prints `"structured": false` with its text.

Claude, CodeBuddy, Codex, DeepSeek Harness, Pi, and OpenCode transcripts have
turns. A tmux pane capture does not, so there `--final` and `--turns` print its
last 40 lines, or the last `--tail` lines, and say so on stderr.

```sh
agent-berth logs 4wqecx --final
agent-berth logs 4wqecx --turns 1 --tools brief
agent-berth logs 4wqecx --final --json | jq -r '.turns[0].final'
```

Claude, CodeBuddy, Codex, and Pi transcripts come from their local JSONL
session files. DeepSeek Harness appends one too, under
`$DSH_HOME/sessions/<cwd>/<session-id>/session.v4.jsonl.zstd`, compressed as a
sequence of zstd frames, one per write; agent-berth decodes the frames in order
and keeps the complete ones when the last is still being written. OpenCode
keeps sessions in a service-owned database with no log file, so `logs` runs
`opencode session export <id>` and renders its messages. When no local or
exported transcript is available — an unsupported provider such as Grok or
Paseo, a missing file, or an OpenCode server that is not running — and the
session resolves to a tmux pane, `logs` falls back to capturing that pane's
scrollback as plain text.

### Send

`agent-berth send <id> "message"` delivers a message to a session, where
`<id>` is the stable short ID shown by `list`. The remaining arguments are
joined with spaces, or read from stdin when none are given. It uses the agent's
own CLI when
that CLI can hand a message to the running session: Codex goes through
`codex queue --thread <id> --message`, which targets the shared app-server the
session is attached to, and OpenCode goes through its prompt API on the shared
background server. Claude, CodeBuddy, Grok, and Pi have no command that reaches
an already-running session, so the message is typed into the tmux pane running
it with `tmux send-keys`: `-l` keeps the text literal, each newline is inserted
with `C-j`, and a final Enter submits it. The session must be the active
program in the pane; a session that is only a background tab is rejected.
Codex desktop sessions are reached like CLI ones when they share the local
app-server. A Claude desktop session has no pane, so a fresh
`claude --model sonnet -p` run delivers the message through the CLI's
cross-session messaging, addressed by the session's name and Claude session
ID. A DeepSeek Harness session the app runs also has no pane and no command
that takes a message, so `send` refuses it; type the message in the app. One
started in a terminal, such as `dsh tui`, is reached through its pane.

A session whose status is `waiting` is refused: it is showing a permission
prompt or a question, so typed text and Enter would answer the dialog, and a
message queued behind it is not what the caller meant either. Answer the
dialog with `agent-berth attach <id>`, or pass `--force` to deliver anyway.

`send` prints `sent to <provider> <session-id> (turns_ended N)`, where N is
the session's turn count before delivery, for a later `wait --after N`.

### Wait

`agent-berth wait <id> [--after N] [--timeout DURATION]` blocks until the
session ends a turn and prints one word on stdout, with the reason on stderr:
`done` (exit 0) once a turn ended and the session is not busy, `waiting`
(exit 2) when it is blocked on a permission prompt or a question, `gone`
(exit 3) when it exited or is no longer active, or `timeout` (exit 4; the
default is `10m`).

Every session carries monotonic `turns_started` and `turns_ended` counts in
`list --json`, `search --json`, and `self --json`. Hook providers (Claude,
CodeBuddy, Codex, Grok) count a start when a turn becomes `running` or
`waiting` and an end on each Stop hook; plugin providers (DeepSeek Harness,
OpenCode, Pi) count the busy/idle edges between their reports. Unlike `status`,
which only holds the latest level, the counts show a turn that began and ended
between two looks, and they survive server restarts.

`wait` returns `done` once `turns_ended` exceeds a baseline: `--after N` when
given, otherwise the count read when `wait` starts, so a turn that ended
before then is missed. Right after `new` the count is 0, so `--after 0` is
safe. `send --wait [--timeout DURATION] <id> "message"` reads the count
before delivering and then waits exactly like `wait --after` that count; its
"sent to" line goes to stderr so stdout holds only the outcome.

```sh
pane=$(agent-berth new claude "Fix the failing test")
id=$(agent-berth search --tmux-pane "$pane" --json | jq -r '.[0].id')
agent-berth wait --after 0 "$id" && agent-berth logs "$id" --final
agent-berth send --wait "$id" "Now run the full suite." && agent-berth logs "$id" --final
```

An agent queues a message sent while it is busy, so `wait` then reports the
end of the turn already in progress; send follow-ups once the previous turn
has ended.

### Delegation

When a tracked agent runs `agent-berth new`, the new window gets
`AGENT_BERTH_DELEGATOR` set to the caller's stable id (`--delegator <id>`
names another, `--no-delegator` leaves it out). The agent's hooks and
plugins run inside that window, so `agent-berth notify` copies the variable
into every report and the server stores it as the session's `delegator`,
shown in `list --json`, `search --json`, and `self --json`. A later report
without it keeps the stored value. For OpenCode 2 the variable is read by
the reporting process: the TUI names the delegator of the sessions it tabs,
and a standalone server names it for the sessions it runs.

`agent-berth search --delegated-by <id>` keeps only the sessions a given
session started, and a plain `search <id>` pattern matches the delegator
too.

The delegate answers with `agent-berth reply "message"`, which resolves the
delegator from `AGENT_BERTH_DELEGATOR`, or else from the delegate's own
stored session, and then behaves exactly like `send` to that id: the same
`--wait`, `--timeout`, `--force`, and `--raw` flags, the waiting refusal,
and the exit codes. It fails with a clear error when there is no delegator.

When the sender of `send` or `reply` is itself a tracked agent (resolved as
`self` does), the message starts with one line naming it, followed by a
blank line:

```
[agent-berth message from 4wqecx (claude)]

Tests pass; the changelog is updated.
```

`--raw` leaves the line out. A human running `send` from a shell pane has no
identity, so their messages carry no header.

### Stop

`agent-berth stop <id>` ends a session by killing the tmux pane that shows
it (`tmux kill-pane`). It refuses a session that no pane shows — a desktop
or Paseo session, one that already exited, or a tab in the background of an
OpenCode TUI — and, without `--force`, a pane whose process also shows or
hosts other sessions, since they would end with it. `--dry-run` prints what
would be killed. The session stays stored and searchable, its transcript
stays readable with `logs`, and `rm` hides it.

### Resume behavior

`resume` selects sessions that are still busy (`running`/`waiting`) with no
live agent process. With `--idle` (default window `20m`, or `--idle 1h`), it also
includes CLI-resumable idle/done sessions, including closed tabs and gracefully
ended sessions, within that window. Open tabs remain excluded even if idle;
an OpenCode shared server staying alive does not count as an open tab. Selected
sessions are grouped by working directory; each directory gets a tmux session
and each agent session gets a window running the provider's resume command.
Pass a `pattern` to pick one session interactively with fzf, `--here` to only
consider sessions in the current directory, and `--dry-run` to print the plan
without starting anything. `list --resumable` shows exactly what `resume`
would start.

## How it works

The hooks installed by `setup` call `agent-berth notify` on agent events
(session start, prompt submit, tool use, permission requests, stop, session
end). Agents that load plugins instead of hooks — DeepSeek Harness, OpenCode,
and Pi — get one installed the same way, and it reports each session's status
on a heartbeat. The DeepSeek Harness plugin is written to
`$DSH_HOME/agent-berth/plugin.mjs` and inserted by a row in
`$DSH_HOME/cordis.patch.yml`, the patch layer the Harness composes into every
profile, so restart the app after `setup` for it to load. It reports the
sessions the app lists. A session archived in the Harness stays out of
`list` while it is idle or done, and is shown again only while it is running
or waiting, whether the app or a terminal booted it. `rm` is remembered the
same way for every provider: an idle or done report, including a heartbeat,
does not undo it, and the session hides again once that turn ends. The server
folds hook and plugin reports into per-session status and persists them in a
redb database, so state survives restarts. Sessions are keyed by provider and
session ID, with the working directory and command line needed to resume.

State lives in `$XDG_STATE_HOME/agent-berth` (`~/.local/state/agent-berth`,
or `%LOCALAPPDATA%\agent-berth` on Windows). The IPC endpoint is
`$XDG_RUNTIME_DIR/agent-berth.sock` on Unix and the `\\.\pipe\agent-berth`
named pipe on Windows.

### Environment variables

| Variable | Effect |
| --- | --- |
| `PASEO_HOME` | Override the local Paseo directory used for routing, streaming, and focus (default `~/.paseo`) |
| `AGENT_BERTH_SOCK` | Override the IPC endpoint (socket path or pipe name) |
| `AGENT_BERTH_TMUX_SOCKET` | Add `tmux -L <name>` to every tmux invocation |
| `AGENT_BERTH_TMUX_CONFIG` | Add `tmux -f <path>` to every tmux invocation |

Provider config locations honor `CLAUDE_CONFIG_DIR`, `CODEBUDDY_CONFIG_DIR`,
`CODEX_HOME`, `DSH_HOME`, `GROK_HOME`, and `PI_CODING_AGENT_DIR`.

## Development

Tasks are defined in `mise.toml` and runnable with [mise](https://mise.jdx.dev):

```sh
mise run            # cargo build
mise run test       # full test suite
mise pre-commit     # cargo fmt --check + typos
```

See [tests/README.md](tests/README.md) for the integration test harness,
including the real-tmux, mock-LLM, and live-client suites.

## License

[MPL-2.0](LICENSE)
