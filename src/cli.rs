use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::duration::parse_idle;
use crate::paths::Context;
use crate::{doctor, list, notify, resume, server, setup};

/// Monitor coding agents and resume their sessions.
#[derive(Debug, Parser)]
#[command(name = "agent-bridge", version, about, propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the bridge server
    Server,
    /// Install the user service and agent hooks
    Setup {
        /// Install hooks only; do not install the user service
        #[arg(long)]
        no_service: bool,
    },
    /// Stop the service and remove agent hooks
    Teardown,
    /// List sessions
    List {
        /// Print JSON
        #[arg(long)]
        json: bool,
        /// Show sessions that resume would start
        #[arg(long)]
        resumable: bool,
        /// Idle window used with --resumable (default: 20m)
        #[arg(long, value_name = "DURATION", requires = "resumable")]
        idle: Option<String>,
    },
    /// Report agent status to the bridge server (used by hooks)
    Notify {
        #[arg(long)]
        provider: String,
    },
    /// Check server, service, and agent hooks
    Doctor,
    /// Resume sessions after the server or host restarts
    Resume {
        /// Idle window for idle sessions (default: 20m)
        #[arg(long, value_name = "DURATION")]
        idle: Option<String>,
        /// Print actions without starting agents
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let ctx = Context::from_env()?;
    match cli.command {
        Command::Server => server::run(&ctx),
        Command::Setup { no_service } => setup::setup(&ctx, no_service),
        Command::Teardown => setup::teardown(&ctx),
        Command::List {
            json,
            resumable,
            idle,
        } => {
            let idle = if resumable {
                Some(parse_idle(idle.as_deref())?)
            } else {
                None
            };
            list::run(&ctx, json, resumable, idle)
        }
        Command::Notify { provider } => notify::run(&ctx, provider),
        Command::Doctor => doctor::run(&ctx),
        Command::Resume { idle, dry_run } => {
            resume::run(&ctx, parse_idle(idle.as_deref())?, dry_run)
        }
    }
}
