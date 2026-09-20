use std::time::Duration;

use anyhow::{Context, Result, bail};

pub fn parse_duration(input: &str) -> Result<Duration> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("duration must not be empty");
    }
    let split = trimmed
        .char_indices()
        .find(|(_, ch)| !ch.is_ascii_digit() && *ch != '.')
        .map(|(i, _)| i)
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    if number.is_empty() {
        bail!("duration {input:?} is missing a number");
    }
    let value: f64 = number
        .parse()
        .with_context(|| format!("invalid duration number in {input:?}"))?;
    if !value.is_finite() || value < 0.0 {
        bail!("duration {input:?} must be a finite non-negative number");
    }
    let factor = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
        "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
        "d" | "day" | "days" => 86400.0,
        other => bail!("unknown duration unit {other:?} in {input:?}"),
    };
    Ok(Duration::from_secs_f64(value * factor))
}

pub fn parse_idle(input: Option<&str>) -> Result<Option<Duration>> {
    input.map(parse_duration).transpose()
}

#[cfg(test)]
#[path = "duration_tests.rs"]
mod tests;
