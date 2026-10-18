use std::io::{self, Write};

use anyhow::Result;

use crate::db;
use crate::hubs::tmux::running_pane;
use crate::paths::{self, Context};
use crate::store::ListedSession;
use crate::tmux::{self, Pane};

pub fn run(
    ctx: &Context,
    json: bool,
    resumable: bool,
    idle: Option<std::time::Duration>,
    here: bool,
) -> Result<()> {
    let mut sessions = db::query_sessions(ctx, resumable, idle)?;
    if here {
        sessions.retain(|session| session.cwd.as_deref().is_some_and(paths::is_current_dir));
    }
    if json {
        println!("{}", to_json(&sessions)?);
        return Ok(());
    }
    if sessions.is_empty() {
        if resumable {
            println!("No resumable sessions.");
        } else {
            println!("No active sessions.");
        }
        return Ok(());
    }
    print_table(&sessions, false)?;
    Ok(())
}

/// Serialize sessions with their stable IDs and the tmux panes they run in,
/// which are computed rather than stored and so are not part of
/// `ListedSession` itself.
pub(crate) fn to_json(sessions: &[ListedSession]) -> Result<String> {
    // Without tmux no session has a pane, so the field is simply omitted.
    let panes = if tmux::available() {
        tmux::list_panes(true).unwrap_or_default()
    } else {
        Vec::new()
    };
    to_json_with_panes(sessions, &panes)
}

/// Like [`to_json`], with tmux panes the caller already listed.
pub(crate) fn to_json_with_panes(sessions: &[ListedSession], panes: &[Pane]) -> Result<String> {
    Ok(serde_json::to_string_pretty(&json_values(
        sessions,
        panes,
        std::process::id(),
    )?)?)
}

/// Like [`to_json`], for a single session printed as a bare object.
pub(crate) fn to_json_object(session: &ListedSession, panes: &[Pane]) -> Result<String> {
    let mut values = json_values(std::slice::from_ref(session), panes, std::process::id())?;
    Ok(serde_json::to_string_pretty(&values.remove(0))?)
}

fn json_values(
    sessions: &[ListedSession],
    panes: &[Pane],
    me: u32,
) -> Result<Vec<serde_json::Value>> {
    sessions
        .iter()
        .map(|session| {
            let mut value = serde_json::to_value(session)?;
            if let Some(map) = value.as_object_mut() {
                map.insert("id".into(), session.id().into());
                if let Some(pane) = running_pane(session, panes, me) {
                    map.insert("tmux_pane".into(), serde_json::to_value(pane)?);
                }
            }
            Ok(value)
        })
        .collect()
}

pub(crate) fn print_table(sessions: &[ListedSession], created: bool) -> Result<()> {
    let headers = [
        "ID",
        "PROVIDER",
        "STATUS",
        "SOURCE",
        "SESSION",
        "TITLE",
        "CWD",
        "PID",
        if created { "CREATED" } else { "AGO" },
    ];
    let rows: Vec<[String; 9]> = sessions
        .iter()
        .map(|session| {
            [
                session.id(),
                session.provider.clone(),
                session.status.as_str().to_string(),
                session.source.as_str().to_string(),
                session.session_id.clone(),
                truncate(session.display_title().unwrap_or("-"), 40),
                session.cwd.clone().unwrap_or_else(|| "-".into()),
                session
                    .pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".into()),
                ago_label(if created {
                    session.created_ms
                } else {
                    session.last_report_ms
                }),
            ]
        })
        .collect();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    let mut out = io::stdout().lock();
    for (i, header) in headers.iter().enumerate() {
        if i > 0 {
            write!(out, "  ")?;
        }
        write!(out, "{header:<width$}", width = widths[i])?;
    }
    writeln!(out)?;
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                write!(out, "  ")?;
            }
            write!(out, "{cell:<width$}", width = widths[i])?;
        }
        writeln!(out)?;
    }
    Ok(())
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub(crate) fn ago_label(last_report_ms: u64) -> String {
    let now = crate::store::now_ms();
    let secs = now.saturating_sub(last_report_ms) / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
