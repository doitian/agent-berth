# agent-berth

Monitor coding agents and resume their sessions.

agent-berth runs a small background server that tracks the status of your AI
coding agent sessions — Claude Code, Codex, Grok, OpenCode, and Pi — through
hooks installed into each agent. When the server or the host restarts, it can
resume the sessions you were working on, each in its own tmux window, grouped
by working directory.

## Features

- **Live status tracking** — see which sessions are `working`, `waiting`,
  `idle`, or `done`, across all supported agents.
- **Session resume** — restart sessions that were interrupted by a reboot or
  server restart, each in its own tmux window via the agent's native resume
  command (`claude --resume`, `codex resume`, …).
- **Attach** — fuzzy-find a running agent pane with fzf and jump straight to
  it in tmux.
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
| `agent-berth setup [--no-service]` | Install the user service and agent hooks |
| `agent-berth teardown` | Stop the service and remove all agent hooks |
| `agent-berth service start\|stop\|restart` | Manage the background server |
| `agent-berth server` | Run the server in the foreground |
| `agent-berth list [--json] [--resumable [--idle 20m] [--here]]` | List sessions |
| `agent-berth resume [pattern] [--idle 20m] [--here] [--dry-run]` | Resume sessions in tmux |
| `agent-berth attach [query] [--preview] [--session] [--dry-run]` | Attach to a running agent pane with fzf |
| `agent-berth rm [patterns...]` | Hide sessions so they are never resumed |
| `agent-berth doctor` | Check server, service, and hook installation |
| `agent-berth notify --provider <name>` | Report agent status (used by hooks) |

### Resume behavior

`resume` selects sessions that are still busy (`working`/`waiting`) with no
live agent process, plus sessions that went idle without a graceful exit
within the idle window (default `20m`, override with `--idle`). Selected
sessions are grouped by working directory; each directory gets a tmux session
and each agent session gets a window running the provider's resume command.
Pass a `pattern` to pick one session interactively with fzf, `--here` to only
consider sessions in the current directory, and `--dry-run` to print the plan
without starting anything. `list --resumable` shows exactly what `resume`
would start.

## How it works

The hooks installed by `setup` call `agent-berth notify` on agent events
(session start, prompt submit, tool use, permission requests, stop, session
end). The server folds these events into per-session status and persists them
in a redb database, so state survives restarts. Sessions are keyed by provider
and session ID, with the working directory and command line needed to resume.

State lives in `$XDG_STATE_HOME/agent-berth` (`~/.local/state/agent-berth`,
or `%LOCALAPPDATA%\agent-berth` on Windows). The IPC endpoint is
`$XDG_RUNTIME_DIR/agent-berth.sock` on Unix and the `\\.\pipe\agent-berth`
named pipe on Windows.

### Environment variables

| Variable | Effect |
| --- | --- |
| `AGENT_BERTH_SOCK` | Override the IPC endpoint (socket path or pipe name) |
| `AGENT_BERTH_TMUX_SOCKET` | Add `tmux -L <name>` to every tmux invocation |
| `AGENT_BERTH_TMUX_CONFIG` | Add `tmux -f <path>` to every tmux invocation |

Provider config locations honor `CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
`GROK_HOME`, and `PI_CODING_AGENT_DIR`.

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
