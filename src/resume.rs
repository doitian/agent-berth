use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

use crate::db;
use crate::paths::{self, Context};
use crate::pick;
use crate::status::Source;
use crate::store::ListedSession;
use crate::tmux;

pub fn run(
    ctx: &Context,
    idle: Option<Duration>,
    pattern: Option<String>,
    here: bool,
    dry_run: bool,
) -> Result<()> {
    let mut sessions = db::query_sessions(ctx, true, idle)?;
    if here {
        sessions.retain(|session| session.cwd.as_deref().is_some_and(paths::is_current_dir));
    }
    if sessions.is_empty() {
        println!("No resumable sessions.");
        return Ok(());
    }

    let sessions = match pattern {
        Some(pattern) => match pick::single(&sessions, &pattern)? {
            Some(session) => vec![session],
            None => return Ok(()),
        },
        None => sessions,
    };

    let mut by_cwd: BTreeMap<PathBuf, Vec<&ListedSession>> = BTreeMap::new();
    let mut skipped = Vec::new();
    for session in &sessions {
        if session.source == Source::Desktop {
            skipped.push(session);
            continue;
        }
        let cwd = PathBuf::from(session.cwd.as_deref().unwrap_or(""));
        by_cwd.entry(cwd).or_default().push(session);
    }

    for session in skipped {
        println!(
            "skipping desktop {} {}",
            session.provider, session.session_id
        );
    }

    if !dry_run && !by_cwd.is_empty() && !tmux::available() {
        anyhow::bail!("tmux is required to resume CLI sessions");
    }

    let mut first_tmux = None;
    for (cwd, group) in &by_cwd {
        let session_name = if dry_run {
            tmux::session_name(cwd)
        } else {
            tmux::ensure_session(cwd)?
        };
        if first_tmux.is_none() {
            first_tmux = Some(session_name.clone());
        }
        for session in group {
            let window = window_name(session);
            if dry_run {
                println!(
                    "resume {} {} in tmux {session_name} window {window} ({})",
                    session.provider,
                    session.session_id,
                    cwd.display()
                );
                continue;
            }
            tmux::new_window(&session_name, &window, cwd, &session.cmdline)?;
            db::mark_removed(ctx, &session.provider, &session.session_id)?;
            println!(
                "resumed {} {} in tmux {session_name} window {window}",
                session.provider, session.session_id
            );
        }
    }

    if !dry_run {
        if let Some(name) = first_tmux {
            if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                tmux::attach_or_switch(&name)?;
            }
        }
    }
    Ok(())
}

fn window_name(session: &ListedSession) -> String {
    let short = session
        .session_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>();
    let name = format!("{}-{short}", session.provider);
    name.chars().take(24).collect()
}
