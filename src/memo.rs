use anyhow::Result;

use crate::db;
use crate::paths::Context;
use crate::pick;
use crate::store::normalize_memo;

pub fn run(ctx: &Context, pattern: Option<&str>, text: &[String], clear: bool) -> Result<()> {
    let sessions = db::query_stored(ctx)?;
    if sessions.is_empty() {
        println!("No sessions.");
        return Ok(());
    }
    let Some(session) = pick::single(&sessions, pattern.unwrap_or(""))? else {
        println!("No session selected.");
        return Ok(());
    };
    if clear {
        db::set_memo(ctx, &session.provider, &session.session_id, "")?;
        println!(
            "cleared memo for {} {}",
            session.provider, session.session_id
        );
        return Ok(());
    }
    if text.is_empty() {
        match &session.memo {
            Some(memo) => println!("{memo}"),
            None => println!("No memo set."),
        }
        return Ok(());
    }
    let memo = normalize_memo(&text.join(" "));
    db::set_memo(ctx, &session.provider, &session.session_id, &memo)?;
    println!(
        "memo for {} {}: {memo}",
        session.provider, session.session_id
    );
    Ok(())
}
