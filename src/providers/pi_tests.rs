use super::*;

#[test]
fn skill_block_title_becomes_slash_command() {
    assert_eq!(
        normalize_title(
            "<skill name=\"review-code\" location=\"C:\\skills\\review-code\\SKILL.md\"> References are relative to C:\\skills\\review-code. # Review </skill>"
        ),
        "/review-code"
    );
}

#[test]
fn multi_line_skill_block_is_transformed() {
    assert_eq!(
        normalize_title("<skill name=\"review-code\" location=\"/s/SKILL.md\">\nbody\n</skill>"),
        "/review-code"
    );
}

#[test]
fn skill_block_with_args_keeps_them() {
    assert_eq!(
        normalize_title(
            "<skill name=\"review-code\" location=\"/s/SKILL.md\">body</skill> User: src/lib.rs"
        ),
        "/review-code User: src/lib.rs"
    );
}

#[test]
fn other_titles_are_untouched() {
    assert_eq!(normalize_title("Fix the hooks"), "Fix the hooks");
    assert_eq!(
        normalize_title("<skill name=\"x\">no location</skill>"),
        "<skill name=\"x\">no location</skill>"
    );
    assert_eq!(
        normalize_title("<skill name=\"unclosed"),
        "<skill name=\"unclosed"
    );
    assert_eq!(
        normalize_title("<skill name=\"\" location=\"/s\">body</skill>"),
        "<skill name=\"\" location=\"/s\">body</skill>"
    );
}
