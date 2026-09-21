use std::io::{self, Write};

use anyhow::Result;

use crate::db;
use crate::paths::{self, Context};
use crate::store::ListedSession;

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
        println!("{}", serde_json::to_string_pretty(&sessions)?);
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
    print_table(&sessions)?;
    Ok(())
}

fn print_table(sessions: &[ListedSession]) -> Result<()> {
    let headers = [
        "PROVIDER", "STATUS", "SOURCE", "SESSION", "TITLE", "CWD", "PID", "AGE",
    ];
    let rows: Vec<[String; 8]> = sessions
        .iter()
        .map(|session| {
            [
                session.provider.clone(),
                session.status.as_str().to_string(),
                session.source.as_str().to_string(),
                session.session_id.clone(),
                truncate(session.title.as_deref().unwrap_or("-"), 40),
                session.cwd.clone().unwrap_or_else(|| "-".into()),
                session
                    .pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".into()),
                age_label(session.last_report_ms),
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

pub(crate) fn age_label(last_report_ms: u64) -> String {
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
