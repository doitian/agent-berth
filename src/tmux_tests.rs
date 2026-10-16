use super::*;
use std::path::Path;

#[test]
fn names_follow_tmux_up() {
    assert_eq!(session_name(Path::new("/work/agent-berth")), "agent-berth");
    assert_eq!(session_name(Path::new("/work/.dotfiles")), "dotfiles");
    assert_eq!(session_name(Path::new("/work/foo.bar:baz")), "foo_bar_baz");
}

#[test]
fn parses_listed_panes() {
    let pane = Pane::parse("%3\t4321\tproject\t0\tclaude\t/work/project").unwrap();
    assert_eq!(pane.id, "%3");
    assert_eq!(pane.pid, 4321);
    assert_eq!(pane.session, "project");
    assert_eq!(pane.window, "0");
    assert_eq!(pane.window_name, "claude");
    assert_eq!(pane.path, "/work/project");
}

#[test]
fn ignores_malformed_panes() {
    assert_eq!(Pane::parse(""), None);
    assert_eq!(Pane::parse("\tnotapid\ts\t0\tw\t/p"), None);
}

#[test]
fn send_message_keys_type_lines_and_submit() {
    assert_eq!(
        message_keys("%7", "first line\nsecond line"),
        vec![
            vec!["send-keys", "-l", "-t", "%7", "--", "first line"],
            vec!["send-keys", "-t", "%7", "C-j"],
            vec!["send-keys", "-l", "-t", "%7", "--", "second line"],
            vec!["send-keys", "-t", "%7", "Enter"],
        ]
        .into_iter()
        .map(|row| row.into_iter().map(str::to_string).collect::<Vec<_>>())
        .collect::<Vec<_>>()
    );
}

#[test]
fn send_message_keys_keep_hyphens_and_normalize_newlines() {
    assert_eq!(
        message_keys("%1", "a\r\n\r\n-b"),
        vec![
            vec!["send-keys", "-l", "-t", "%1", "--", "a"],
            vec!["send-keys", "-t", "%1", "C-j"],
            vec!["send-keys", "-t", "%1", "C-j"],
            vec!["send-keys", "-l", "-t", "%1", "--", "-b"],
            vec!["send-keys", "-t", "%1", "Enter"],
        ]
        .into_iter()
        .map(|row| row.into_iter().map(str::to_string).collect::<Vec<_>>())
        .collect::<Vec<_>>()
    );
}

#[test]
fn send_message_keys_escape_a_trailing_semicolon() {
    assert_eq!(
        message_keys("%2", "fix;\n;"),
        vec![
            vec!["send-keys", "-l", "-t", "%2", "--", "fix\\;"],
            vec!["send-keys", "-t", "%2", "C-j"],
            vec!["send-keys", "-l", "-t", "%2", "--", "\\;"],
            vec!["send-keys", "-t", "%2", "Enter"],
        ]
        .into_iter()
        .map(|row| row.into_iter().map(str::to_string).collect::<Vec<_>>())
        .collect::<Vec<_>>()
    );
}

#[test]
fn send_message_rejects_control_characters() {
    let error = send_message("%3", "a\x1bb").unwrap_err().to_string();
    assert!(error.contains("control character"), "{error}");
}

#[test]
fn send_message_keys_send_tabs_as_spaces() {
    assert_eq!(
        message_keys("%3", "a\tb"),
        vec![
            vec!["send-keys", "-l", "-t", "%3", "--", "a b"],
            vec!["send-keys", "-t", "%3", "Enter"],
        ]
        .into_iter()
        .map(|row| row.into_iter().map(str::to_string).collect::<Vec<_>>())
        .collect::<Vec<_>>()
    );
}

#[test]
#[cfg(not(windows))]
fn preview_command_quotes_arguments() {
    assert_eq!(quote_arg("tmux"), "tmux");
    assert_eq!(quote_arg("a b"), "\"a b\"");
}

#[test]
#[cfg(windows)]
fn preview_runs_in_powershell() {
    let preview = preview_command().replace("{1}", "'%3'");
    let argument = "C:\\agent's config\\$literal & (test).conf";
    let script = format!(
        "function tmux {{ \
             if (($args -join ' ') -notlike '*capture-pane -p -e -t %3') {{ throw 'invalid arguments' }}; \
             1..45 | ForEach-Object {{ 'line ' + $_ }}; \
             [char]27 + '[32m' + [char]0x4e2d + [char]0x6587 + [char]27 + '[0m' \
         }}; \
         if (({}) -cne $env:EXPECTED_ARGUMENT) {{ throw 'invalid quoting' }}; \
         {preview}",
        quote_arg(argument),
    );
    for (height, expected_lines) in [(Some("3"), 3), (None, 40)] {
        let mut shell = PREVIEW_SHELL.split_whitespace();
        let mut command = Command::new(shell.next().unwrap());
        command
            .args(shell)
            .arg(&script)
            .env("EXPECTED_ARGUMENT", argument)
            .env_remove("FZF_PREVIEW_LINES");
        if let Some(height) = height {
            command.env("FZF_PREVIEW_LINES", height);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), expected_lines);
        assert_eq!(lines[0], format!("line {}", 47 - expected_lines));
        assert_eq!(lines.last().unwrap(), &"\x1b[32m中文\x1b[0m");
    }
}
