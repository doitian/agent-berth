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
