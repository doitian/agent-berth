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
fn idle_is_explicit() {
    assert_eq!(parse_idle(None).unwrap(), None);
    assert_eq!(
        parse_idle(Some("30m")).unwrap(),
        Some(Duration::from_secs(30 * 60))
    );
}

#[test]
fn formats_largest_exact_unit() {
    assert_eq!(format_duration(Duration::from_secs(1200)), "20m");
    assert_eq!(format_duration(Duration::from_secs(3600)), "1h");
    assert_eq!(format_duration(Duration::from_secs(86400)), "1d");
    assert_eq!(format_duration(Duration::from_secs(45)), "45s");
    assert_eq!(format_duration(Duration::from_secs(90)), "90s");
    assert_eq!(format_duration(Duration::ZERO), "0s");
}

#[test]
fn format_round_trips_through_parse() {
    for secs in [1200, 3600, 86400, 45] {
        let duration = Duration::from_secs(secs);
        assert_eq!(
            parse_duration(&format_duration(duration)).unwrap(),
            duration
        );
    }
}
