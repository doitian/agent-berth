//! Block until a session ends a turn. `status` is a level, so a turn shorter
//! than a poll interval never shows there; the monotonic `turns_ended`
//! counter does, compared against a baseline taken before the turn began.
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::db;
use crate::duration::format_duration;
use crate::paths::Context as AppContext;
use crate::pick;
use crate::status::AgentStatus;
use crate::store::{ListedSession, now_ms};

const POLL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A turn ended after the baseline and the session is not busy.
    Done,
    /// The session is blocked on a permission prompt or a question.
    Waiting,
    /// The session exited or is no longer active.
    Gone,
    Timeout,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Waiting => "waiting",
            Self::Gone => "gone",
            Self::Timeout => "timeout",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            Self::Done => 0,
            Self::Waiting => 2,
            Self::Gone => 3,
            Self::Timeout => 4,
        }
    }
}

/// Waits for the session with stable id `id`. The baseline is `after` when
/// given, otherwise the session's `turns_ended` as it is now.
pub fn run(ctx: &AppContext, id: &str, after: Option<u64>, timeout: Duration) -> Result<Outcome> {
    let sessions = db::query_stored(ctx)?;
    let session = pick::by_id(&sessions, id)?.clone();
    let baseline = after.unwrap_or(session.turns_ended);
    wait_for(ctx, &session, baseline, timeout)
}

/// Polls until the session ends a turn past `baseline`, blocks, goes away,
/// or `timeout` passes. Prints the outcome word on stdout and the reason on
/// stderr, so scripts read stdout alone.
pub fn wait_for(
    ctx: &AppContext,
    session: &ListedSession,
    baseline: u64,
    timeout: Duration,
) -> Result<Outcome> {
    let deadline = Instant::now() + timeout;
    let mut latest;
    let outcome = loop {
        latest = db::query_stored(ctx)?.into_iter().find(|candidate| {
            candidate.provider == session.provider && candidate.session_id == session.session_id
        });
        if let Some(outcome) = check(latest.as_ref(), baseline, now_ms()) {
            break outcome;
        }
        let now = Instant::now();
        if now >= deadline {
            break Outcome::Timeout;
        }
        std::thread::sleep(POLL.min(deadline - now));
    };
    eprintln!(
        "{}",
        describe(outcome, session, latest.as_ref(), baseline, timeout)
    );
    println!("{}", outcome.as_str());
    Ok(outcome)
}

/// What one poll settles, if anything. A finished turn wins over an exited
/// session, since its output is there to read; a dead session wins over a
/// stale `waiting` level that no process will ever answer.
fn check(session: Option<&ListedSession>, baseline: u64, now: u64) -> Option<Outcome> {
    let Some(session) = session else {
        return Some(Outcome::Gone);
    };
    if session.turns_ended > baseline && !session.status.is_busy() {
        return Some(Outcome::Done);
    }
    if session.exited || !session.is_active(now) {
        return Some(Outcome::Gone);
    }
    if session.status == AgentStatus::Waiting {
        return Some(Outcome::Waiting);
    }
    None
}

fn describe(
    outcome: Outcome,
    session: &ListedSession,
    latest: Option<&ListedSession>,
    baseline: u64,
    timeout: Duration,
) -> String {
    let name = format!("{} {}", session.provider, session.id());
    let ended = latest.map_or(session.turns_ended, |latest| latest.turns_ended);
    match outcome {
        Outcome::Done => format!("{name} ended a turn (turns_ended {ended}, was {baseline})"),
        Outcome::Waiting => format!("{name} is waiting on a permission prompt or a question"),
        Outcome::Gone => match latest {
            Some(latest) if latest.exited => format!("{name} has exited"),
            Some(_) => format!("{name} is no longer active"),
            None => format!("{name} is no longer stored"),
        },
        Outcome::Timeout => {
            let status = latest.map_or("unknown", |latest| latest.status.as_str());
            format!(
                "{name} did not end a turn within {} (status {status}, turns_ended {ended}, was {baseline})",
                format_duration(timeout)
            )
        }
    }
}

#[cfg(test)]
#[path = "wait_tests.rs"]
mod tests;
