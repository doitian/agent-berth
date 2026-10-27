use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::duration::{parse_duration, parse_idle};
use crate::paths::Context;
use crate::transcript::Tools;
use crate::{
    attach, doctor, list, logs, memo, new, notify, reply, resume, rm, search, self_cmd, send,
    server, service, setup, stats, stop, tui, wait,
};

/// Monitor coding agents and resume their sessions.
#[derive(Debug, Parser)]
#[command(name = "agent-berth", version, about, propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Launch the interactive TUI (default when no subcommand is given)
    Tui {
        /// Attach to a tmux pane already running the TUI, creating a new
        /// window in the default session when none exists
        #[arg(long)]
        tmux: bool,
    },
    /// Start the server
    Server,
    /// Install the user service and agent hooks
    Setup {
        /// Install hooks only; do not install the user service
        #[arg(long)]
        no_service: bool,
    },
    /// Stop the service and remove agent hooks
    Teardown,
    /// Manage the background server service
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// List sessions
    List {
        /// Print JSON
        #[arg(long)]
        json: bool,
        /// Show sessions that resume would start
        #[arg(long)]
        resumable: bool,
        /// Only include idle sessions within this window (default: 20m)
        #[arg(
            long,
            value_name = "DURATION",
            requires = "resumable",
            num_args = 0..=1,
            default_missing_value = "20m"
        )]
        idle: Option<String>,
        /// Restrict to sessions in the current directory
        #[arg(long, requires = "resumable")]
        here: bool,
    },
    /// Search stored sessions across all providers, including inactive sessions
    Search {
        /// Match all terms against provider, session ID, title, memo, status, directory, or branch
        patterns: Vec<String>,
        /// Only include sessions created within this window (e.g. 3h, 2d)
        #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
        created_in: Option<std::time::Duration>,
        /// Only include sessions started with `new` by the session with this
        /// stable id
        #[arg(long, value_name = "ID")]
        delegated_by: Option<String>,
        /// Only include the session this tmux pane shows now (e.g. %7, or
        /// "$TMUX_PANE" for the current pane)
        #[arg(long, value_name = "PANE_ID")]
        tmux_pane: Option<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Print the stable id of the current agent session
    #[command(name = "self")]
    Self_ {
        /// Print the session as JSON instead of the bare id
        #[arg(long)]
        json: bool,
    },
    /// Aggregate active session counts by status and provider
    Stats {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Report agent status to the server (used by hooks)
    Notify {
        #[arg(long)]
        provider: String,
    },
    /// Attach to a running agent in a tmux pane
    Attach {
        /// Initial fzf query
        query: Option<String>,
        /// Show the pane preview (toggle with ctrl-t)
        #[arg(short, long)]
        preview: bool,
        /// Only consider panes in the current tmux session
        #[arg(short, long)]
        session: bool,
        /// Print candidate sessions without attaching
        #[arg(long)]
        dry_run: bool,
    },
    /// Check server, service, and agent hooks
    Doctor,
    /// Send a message to a session
    ///
    /// Prints `sent to <provider> <session-id> (turns_ended N)`, where N is
    /// the session's count before delivery: pass it to `wait --after` to
    /// wait for the turn this message starts. When the sender is itself a
    /// tracked agent, the message starts with a line naming it.
    Send {
        /// Stable session ID shown in `list` and `search`
        id: String,
        /// Message to send; remaining arguments are joined with spaces. Read
        /// from stdin when omitted.
        #[arg(allow_hyphen_values = true)]
        message: Vec<String>,
        #[command(flatten)]
        delivery: Delivery,
    },
    /// Send a message to the session that started this agent with `new`
    ///
    /// The delegator is read from AGENT_BERTH_DELEGATOR in the environment,
    /// or else from this agent's own session as its hooks reported it. The
    /// message is then sent exactly as `send` would, including the sender
    /// line and the exit codes.
    Reply {
        /// Message to send; remaining arguments are joined with spaces. Read
        /// from stdin when omitted.
        #[arg(allow_hyphen_values = true)]
        message: Vec<String>,
        #[command(flatten)]
        delivery: Delivery,
    },
    /// Wait for a session to end a turn
    ///
    /// Polls the session's monotonic `turns_ended` count about twice a
    /// second and prints one word on stdout, with the reason on stderr:
    /// `done` once a turn ended after the baseline and the session is not
    /// busy (exit 0), `waiting` when it is blocked on a permission prompt or
    /// a question (exit 2), `gone` when it exited or is no longer active
    /// (exit 3), or `timeout` (exit 4).
    ///
    /// Without --after, the baseline is the count read when `wait` starts,
    /// so a turn that ended before then is missed. Callers that `send`
    /// first should pass the `turns_ended` value `send` prints, or use `send
    /// --wait`. Right after `new`, `--after 0` is safe.
    Wait {
        /// Stable session ID shown in `list` and `search`
        id: String,
        /// Return once `turns_ended` exceeds this count instead of the count
        /// read when `wait` starts
        #[arg(long, value_name = "N")]
        after: Option<u64>,
        /// Give up after this long, e.g. 90s or 15m
        #[arg(
            long,
            value_name = "DURATION",
            value_parser = parse_duration,
            default_value = "10m"
        )]
        timeout: Duration,
    },
    /// Start an agent in a new window of the current directory's tmux session
    ///
    /// Prints the new pane id, or with --wait the stable session id once the
    /// agent's first report arrives: exit 4 when none arrives in time, exit 1
    /// when the pane closes first. The "started" line on stderr names the
    /// pane in both modes.
    New {
        /// Wait for the agent's first report and print its stable session id
        /// instead of the pane id
        #[arg(long)]
        wait: bool,
        /// Stable id the agent gets as AGENT_BERTH_DELEGATOR, to `reply` to,
        /// instead of the session running `new`
        #[arg(long, value_name = "ID", conflicts_with = "no_delegator")]
        delegator: Option<String>,
        /// Start the agent without a delegator in its environment
        #[arg(long)]
        no_delegator: bool,
        /// Give up waiting after this long, e.g. 90s or 5m
        #[arg(
            long,
            value_name = "DURATION",
            value_parser = parse_duration,
            default_value = "2m",
            requires = "wait"
        )]
        timeout: Duration,
        /// Agent command and its arguments, e.g. `claude --model opus`
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// End a session by killing the tmux pane that shows it
    ///
    /// The session stays stored and searchable, its transcript stays readable
    /// with `logs`, and `rm` hides it.
    Stop {
        /// Stable session ID shown in `list` and `search`
        id: String,
        /// Kill the pane even when it also hosts other sessions, ending them too
        #[arg(long)]
        force: bool,
        /// Print what would be killed without killing it
        #[arg(long)]
        dry_run: bool,
    },
    /// Resume sessions after the server or host restarts
    Resume {
        /// Select the session matching this pattern with fzf
        pattern: Option<String>,
        /// Include idle sessions within this window (default: 20m)
        #[arg(
            long,
            value_name = "DURATION",
            num_args = 0..=1,
            default_missing_value = "20m"
        )]
        idle: Option<String>,
        /// Restrict to sessions in the current directory
        #[arg(long)]
        here: bool,
        /// Print actions without starting agents
        #[arg(long)]
        dry_run: bool,
    },
    /// Print a session's full transcript
    Logs {
        /// Session ID or pattern
        selector: String,
        /// Print only the agent's latest reply, and any tool call it is
        /// waiting on
        #[arg(long = "final", conflicts_with_all = ["turns", "tools"])]
        last_reply: bool,
        /// Print the last N turns, each starting with its prompt, without the
        /// context the agent's harness injected
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
        turns: Option<u64>,
        /// How much of each tool call and result to show
        #[arg(long, value_enum, value_name = "MODE")]
        tools: Option<Tools>,
        /// Only print the last N lines
        #[arg(long, value_name = "LINES", conflicts_with = "json")]
        tail: Option<usize>,
        /// Print the selected turns as JSON
        #[arg(long)]
        json: bool,
    },
    /// Set or show a session's memo, shown instead of its detected title
    Memo {
        /// Session ID or pattern to select the session with fzf
        pattern: Option<String>,
        /// Memo text; remaining arguments are joined with spaces. With no
        /// text, prints the current memo.
        #[arg(num_args = 0..)]
        text: Vec<String>,
        /// Clear the memo
        #[arg(long, conflicts_with = "text")]
        clear: bool,
    },
    /// Remove sessions so they stay hidden until they are running or waiting
    #[command(alias = "remove")]
    Rm {
        /// Only consider sessions matching these terms
        patterns: Vec<String>,
    },
}

/// How `send` and `reply` deliver a message.
#[derive(Debug, clap::Args)]
struct Delivery {
    /// Deliver even when the session is waiting on a permission prompt or
    /// a question, where typed text and Enter would answer the dialog
    #[arg(long)]
    force: bool,
    /// Leave out the line naming the sending agent
    #[arg(long)]
    raw: bool,
    /// After delivering, wait for the turn to end exactly as `wait --after
    /// N` would, with N read before delivery; the "sent to" line then goes
    /// to stderr and stdout holds only the outcome word
    #[arg(long)]
    wait: bool,
    /// Give up waiting after this long, e.g. 90s or 15m
    #[arg(
        long,
        value_name = "DURATION",
        value_parser = parse_duration,
        default_value = "10m",
        requires = "wait"
    )]
    timeout: Duration,
}

impl Delivery {
    fn options(&self) -> send::Options {
        send::Options {
            force: self.force,
            raw: self.raw,
            wait: self.wait.then_some(self.timeout),
        }
    }
}

/// Ends the process with a command's exit code, so scripts can branch on it
/// without parsing stdout.
fn exit(code: i32) -> Result<()> {
    match code {
        0 => Ok(()),
        code => std::process::exit(code),
    }
}

fn delivered(outcome: Option<wait::Outcome>) -> Result<()> {
    match outcome {
        Some(outcome) => exit(outcome.exit_code()),
        None => Ok(()),
    }
}

#[derive(Debug, Subcommand)]
enum ServiceAction {
    /// Start the background server
    Start,
    /// Stop the background server
    Stop,
    /// Restart the background server
    Restart,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let ctx = Context::from_env()?;
    let Some(command) = cli.command else {
        if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            return list::run(&ctx, false, false, None, false);
        }
        return tui::run(&ctx);
    };
    match command {
        Command::Tui { tmux } => {
            if tmux {
                return tui::run_tmux(&ctx);
            }
            if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                anyhow::bail!("tui requires a terminal");
            }
            tui::run(&ctx)
        }
        Command::Server => server::run(&ctx),
        Command::Setup { no_service } => setup::setup(&ctx, no_service),
        Command::Teardown => setup::teardown(&ctx),
        Command::Service { action } => match action {
            ServiceAction::Start => service::start(&ctx),
            ServiceAction::Stop => service::stop(&ctx),
            ServiceAction::Restart => service::restart(&ctx),
        },
        Command::List {
            json,
            resumable,
            idle,
            here,
        } => {
            let idle = if resumable {
                parse_idle(idle.as_deref())?
            } else {
                None
            };
            list::run(&ctx, json, resumable, idle, here)
        }
        Command::Search {
            patterns,
            created_in,
            delegated_by,
            tmux_pane,
            json,
        } => search::run(
            &ctx,
            &patterns,
            created_in,
            delegated_by.as_deref(),
            tmux_pane.as_deref(),
            json,
        ),
        Command::Self_ { json } => self_cmd::run(&ctx, json),
        Command::Stats { json } => stats::run(&ctx, json),
        Command::Notify { provider } => notify::run(&ctx, provider),
        Command::Attach {
            query,
            preview,
            session,
            dry_run,
        } => attach::run(&ctx, query, preview, session, dry_run),
        Command::Doctor => doctor::run(&ctx),
        Command::Send {
            id,
            message,
            delivery,
        } => delivered(send::run(&ctx, &id, &message, delivery.options())?),
        Command::Reply { message, delivery } => {
            delivered(reply::run(&ctx, &message, delivery.options())?)
        }
        Command::Wait { id, after, timeout } => {
            exit(wait::run(&ctx, &id, after, timeout)?.exit_code())
        }
        Command::New {
            wait,
            delegator,
            no_delegator,
            timeout,
            command,
        } => {
            let delegator = match delegator {
                Some(id) => new::Delegator::Named(id),
                None if no_delegator => new::Delegator::Unset,
                None => new::Delegator::Caller,
            };
            exit(new::run(
                &ctx,
                &command,
                wait.then_some(timeout),
                delegator,
            )?)
        }
        Command::Stop { id, force, dry_run } => stop::run(&ctx, &id, force, dry_run),
        Command::Resume {
            pattern,
            idle,
            here,
            dry_run,
        } => resume::run(&ctx, parse_idle(idle.as_deref())?, pattern, here, dry_run),
        Command::Rm { patterns } => rm::run(&ctx, patterns),
        Command::Memo {
            pattern,
            text,
            clear,
        } => memo::run(&ctx, pattern.as_deref(), &text, clear),
        Command::Logs {
            selector,
            last_reply,
            turns,
            tools,
            tail,
            json,
        } => {
            let select = match (last_reply, turns) {
                (true, _) => logs::Select::Final,
                (false, Some(count)) => logs::Select::Turns(count as usize),
                (false, None) => logs::Select::All,
            };
            let options = logs::Options {
                select,
                tools: tools.unwrap_or_default(),
                tail,
                json,
            };
            logs::run(&ctx, &selector, options)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_parses_pattern_text_and_clear() {
        let cli = Cli::try_parse_from(["agent-berth", "memo", "abc", "fix", "it"]).unwrap();
        let Some(Command::Memo {
            pattern,
            text,
            clear,
        }) = cli.command
        else {
            panic!("expected memo command");
        };
        assert_eq!(pattern.as_deref(), Some("abc"));
        assert_eq!(text, ["fix", "it"]);
        assert!(!clear);

        let cli = Cli::try_parse_from(["agent-berth", "memo", "abc", "--clear"]).unwrap();
        let Some(Command::Memo { text, clear, .. }) = cli.command else {
            panic!("expected memo command");
        };
        assert!(clear);
        assert!(text.is_empty());

        // Hyphen-leading text needs the `--` separator so flags still parse.
        let cli = Cli::try_parse_from(["agent-berth", "memo", "abc", "--", "-fix"]).unwrap();
        let Some(Command::Memo { text, .. }) = cli.command else {
            panic!("expected memo command");
        };
        assert_eq!(text, ["-fix"]);
        assert!(Cli::try_parse_from(["agent-berth", "memo", "abc", "x", "--clear"]).is_err());
    }
}
