use anyhow::Result;

use crate::db;
use crate::paths::Context;
use crate::pick;

pub fn run(ctx: &Context, patterns: Vec<String>) -> Result<()> {
    let sessions = db::query_all(ctx)?;
    if sessions.is_empty() {
        println!("No sessions.");
        return Ok(());
    }
    let selected = pick::multi(&sessions, &patterns)?;
    if selected.is_empty() {
        println!("No sessions removed.");
        return Ok(());
    }
    for session in &selected {
        db::mark_removed(ctx, &session.provider, &session.session_id)?;
        println!("removed {} {}", session.provider, session.session_id);
    }
    Ok(())
}
