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

use crate::transcript::{Record, turns};

fn session(status: &str) -> ListedSession {
    serde_json::from_value(serde_json::json!({
        "provider":"claude", "session_id":"s", "status":status, "source":"cli",
        "cwd":"/work", "cmdline":[], "pid":std::process::id(), "created_ms":1,
        "last_report_ms":2, "kind":"hook"
    }))
    .unwrap()
}

fn record(entries: Vec<Entry>, starts_turn: bool, ends_turn: bool) -> Record {
    Record {
        entries,
        starts_turn,
        ends_turn,
    }
}

fn prompt(text: &str) -> Entry {
    Entry::Prompt { text: text.into() }
}

fn text(text: &str) -> Entry {
    Entry::Text { text: text.into() }
}

fn call(command: &str) -> Entry {
    Entry::ToolCall {
        name: "Bash".into(),
        input: serde_json::json!({ "command": command }).to_string(),
    }
}

fn result(text: &str) -> Entry {
    Entry::ToolResult { text: text.into() }
}

/// Two finished turns, each with injected context, tool work, and a reply.
fn conversation() -> Conversation {
    Conversation::Turns(turns([
        record(vec![prompt("first task")], true, false),
        record(
            vec![Entry::Context {
                text: "skill body".into(),
            }],
            false,
            false,
        ),
        record(vec![call("ls")], false, false),
        record(vec![result("a\nb\nc")], false, false),
        record(vec![text("First done.")], false, true),
        record(vec![prompt("second task")], true, false),
        record(vec![text("Working on it.")], false, false),
        record(vec![call("cargo test")], false, false),
        record(vec![result("ok")], false, false),
        record(vec![text("All tests pass.")], false, false),
        record(vec![text("Second done.")], false, true),
    ]))
}

fn run(conversation: &Conversation, status: &str, options: Options) -> Output {
    output(conversation, &session(status), options, 3)
}

fn select(select: Select) -> Options {
    Options {
        select,
        ..Options::default()
    }
}

#[test]
fn final_prints_only_the_latest_reply() {
    let output = run(&conversation(), "done", select(Select::Final));
    assert_eq!(output.stdout, "All tests pass.\n\nSecond done.");
    assert!(output.notes.is_empty(), "{:?}", output.notes);
}

#[test]
fn final_shows_the_call_a_waiting_agent_is_blocked_on() {
    let conversation = Conversation::Turns(turns([
        record(vec![prompt("clean up")], true, false),
        record(
            vec![text("I need to delete the build directory.")],
            false,
            false,
        ),
        record(vec![call("rm -rf target")], false, false),
    ]));
    let output = run(&conversation, "waiting", select(Select::Final));
    assert_eq!(
        output.stdout,
        "I need to delete the build directory.\n\nPending: Bash rm -rf target"
    );
    let id = session("waiting").id();
    assert_eq!(
        output.notes,
        [format!(
            "claude {id} is waiting on a permission prompt or a question"
        )]
    );
}

#[test]
fn final_notes_a_prompt_without_a_reply_and_prints_nothing() {
    let conversation = Conversation::Turns(turns([
        record(vec![prompt("first")], true, false),
        record(vec![text("Done.")], false, true),
        record(vec![prompt("follow up")], true, false),
    ]));
    let output = run(&conversation, "running", select(Select::Final));
    assert_eq!(output.stdout, "");
    assert!(
        output
            .notes
            .iter()
            .any(|note| note.ends_with("has not replied to its latest prompt yet")),
        "{:?}",
        output.notes
    );
}

#[test]
fn turns_print_the_latest_turns_without_injected_context() {
    let output = run(&conversation(), "done", select(Select::Turns(1)));
    assert!(
        output.stdout.starts_with("You: second task"),
        "{}",
        output.stdout
    );
    assert!(!output.stdout.contains("first task"));
    let both = run(&conversation(), "done", select(Select::Turns(2)));
    assert!(both.stdout.starts_with("You: first task"));
    assert!(!both.stdout.contains("skill body"));
    // A whole transcript keeps the context, as before.
    let all = run(&conversation(), "done", Options::default());
    assert!(all.stdout.contains("You: skill body"));
}

#[test]
fn tools_brief_and_none_shrink_tool_traffic() {
    let brief = run(
        &conversation(),
        "done",
        Options {
            select: Select::Turns(2),
            tools: Tools::Brief,
            ..Options::default()
        },
    );
    assert!(brief.stdout.contains("Tool: Bash ls"), "{}", brief.stdout);
    assert!(
        brief.stdout.contains("Result: a (+2 lines)"),
        "{}",
        brief.stdout
    );
    assert!(!brief.stdout.contains("Input:"));
    let none = run(
        &conversation(),
        "done",
        Options {
            select: Select::Turns(2),
            tools: Tools::None,
            ..Options::default()
        },
    );
    assert!(!none.stdout.contains("Tool:"), "{}", none.stdout);
    assert!(!none.stdout.contains("Result:"));
    assert!(none.stdout.contains("Assistant: Second done."));
}

#[test]
fn json_lists_selected_turns_with_their_final_reply() {
    let output = run(
        &conversation(),
        "done",
        Options {
            select: Select::Turns(1),
            tools: Tools::None,
            json: true,
            ..Options::default()
        },
    );
    let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(value["structured"], true);
    assert_eq!(value["status"], "done");
    let turns = value["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["index"], 1);
    assert_eq!(turns[0]["complete"], true);
    assert_eq!(turns[0]["prompt"], "second task");
    assert_eq!(turns[0]["final"], "All tests pass.\n\nSecond done.");
    let kinds: Vec<&str> = turns[0]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["prompt", "text", "text", "text"]);
}

#[test]
fn a_pane_capture_falls_back_to_its_last_lines() {
    let capture = Conversation::Text {
        text: (1..=50)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
        origin: "tmux pane %7".into(),
    };
    let output = run(&capture, "done", select(Select::Final));
    assert_eq!(output.stdout.lines().count(), FALLBACK_LINES);
    assert!(output.stdout.ends_with("line 50"));
    assert_eq!(
        output.notes,
        ["tmux pane %7 has no structured transcript; showing its last 40 lines"]
    );
    let json = run(
        &capture,
        "done",
        Options {
            select: Select::Turns(1),
            tail: Some(3),
            json: true,
            ..Options::default()
        },
    );
    let value: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!(value["structured"], false);
    assert_eq!(value["origin"], "tmux pane %7");
    assert_eq!(value["text"], "line 48\nline 49\nline 50");
}
