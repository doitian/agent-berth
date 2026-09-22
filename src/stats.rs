use std::io::{self, Write};

use anyhow::Result;

use crate::db;
use crate::paths::Context;
use crate::store::ProviderStats;

pub fn run(ctx: &Context, json: bool) -> Result<()> {
    let stats = db::query_stats(ctx)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
        return Ok(());
    }
    if stats.is_empty() {
        println!("No active sessions.");
        return Ok(());
    }
    print_table(&stats)?;
    Ok(())
}

fn print_table(stats: &[ProviderStats]) -> Result<()> {
    let headers = ["PROVIDER", "RUNNING", "WAITING", "IDLE", "DONE", "TOTAL"];
    let mut rows: Vec<[String; 6]> = stats
        .iter()
        .map(|row| row_cells(&row.provider, row))
        .collect();
    let total = total_row(stats);
    rows.push(row_cells("total", &total));
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

fn row_cells(label: &str, stats: &ProviderStats) -> [String; 6] {
    [
        label.to_string(),
        stats.running.to_string(),
        stats.waiting.to_string(),
        stats.idle.to_string(),
        stats.done.to_string(),
        stats.total.to_string(),
    ]
}

fn total_row(stats: &[ProviderStats]) -> ProviderStats {
    let mut total = ProviderStats::default();
    for row in stats {
        total.running += row.running;
        total.waiting += row.waiting;
        total.idle += row.idle;
        total.done += row.done;
        total.total += row.total;
    }
    total
}

#[cfg(test)]
#[path = "stats_tests.rs"]
mod tests;
