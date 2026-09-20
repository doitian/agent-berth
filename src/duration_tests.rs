use super::*;
use std::time::Duration;

#[test]
fn parses_units() {
    assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
    assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
    assert_eq!(parse_duration("20m").unwrap(), Duration::from_secs(1200));
    assert_eq!(parse_duration("1.5h").unwrap(), Duration::from_secs(5400));
    assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
}

#[test]
fn rejects_bad_input() {
    assert!(parse_duration("").is_err());
    assert!(parse_duration("m").is_err());
    assert!(parse_duration("20x").is_err());
    assert!(parse_duration("-1m").is_err());
}

#[test]
fn default_idle_is_twenty_minutes() {
    assert_eq!(parse_idle(None).unwrap(), Duration::from_secs(20 * 60));
    assert_eq!(
        parse_idle(Some("30m")).unwrap(),
        Duration::from_secs(30 * 60)
    );
}
