use super::*;

#[test]
fn tail_keeps_the_last_lines() {
    assert_eq!(tail_lines("a\nb\nc", 2), "b\nc");
    assert_eq!(tail_lines("a\nb\nc", 5), "a\nb\nc");
    assert_eq!(tail_lines("a\nb\nc", 0), "");
}

#[test]
fn tail_ignores_a_trailing_newline() {
    assert_eq!(tail_lines("a\nb\nc\n", 1), "c");
}

#[test]
fn tail_of_empty_text_is_empty() {
    assert_eq!(tail_lines("", 3), "");
}
