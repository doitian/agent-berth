use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::duration::parse_idle;
use crate::paths::Context;
use crate::{attach, doctor, list, notify, resume, rm, server, service, setup, tui};

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
    Tui,
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
    /// Report agent status to the server (used by hooks)
    Notify {
        #[arg(long)]
        provider: String,
    },
    /// Attach to a running agent in tmux
    Attach {
        /// Initial fzf query
        query: Option<String>,
        /// Show the pane preview (toggle with ctrl-t)
        #[arg(short, long)]
        preview: bool,
        /// Only consider panes in the current tmux session
        #[arg(short, long)]
        session: bool,
        /// Print candidate panes without attaching
        #[arg(long)]
        dry_run: bool,
    },
    /// Check server, service, and agent hooks
    Doctor,
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
    /// Remove sessions so they are hidden and never resumed
    #[command(alias = "remove")]
    Rm {
        /// Only consider sessions matching these terms
        patterns: Vec<String>,
    },
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
        return tui::run(&ctx);
    };
    match command {
        Command::Tui => tui::run(&ctx),
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
        Command::Notify { provider } => notify::run(&ctx, provider),
        Command::Attach {
            query,
            preview,
            session,
            dry_run,
        } => attach::run(&ctx, query, preview, session, dry_run),
        Command::Doctor => doctor::run(&ctx),
        Command::Resume {
            pattern,
            idle,
            here,
            dry_run,
        } => resume::run(&ctx, parse_idle(idle.as_deref())?, pattern, here, dry_run),
        Command::Rm { patterns } => rm::run(&ctx, patterns),
    }
}
