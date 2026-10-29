use super::*;
use serde_json::json;
use tempfile::tempdir;

use crate::paths::Context as AppContext;
use crate::transcript::Entry;

/// One zstd frame per row, exactly as the Harness appends a session log: a
/// session header, a user prompt, an assistant turn, a tool result, and the
/// turn end.
const ROWS: &[&[u8]] = &[
    &[
        0x28, 0xb5, 0x2f, 0xfd, 0x20, 0x3a, 0xd1, 0x01, 0x00, 0x7b, 0x22, 0x74, 0x79, 0x70, 0x65,
        0x22, 0x3a, 0x22, 0x73, 0x65, 0x73, 0x73, 0x69, 0x6f, 0x6e, 0x22, 0x2c, 0x22, 0x76, 0x65,
        0x72, 0x73, 0x69, 0x6f, 0x6e, 0x22, 0x3a, 0x34, 0x2c, 0x22, 0x69, 0x64, 0x22, 0x3a, 0x22,
        0x73, 0x31, 0x22, 0x2c, 0x22, 0x63, 0x77, 0x64, 0x22, 0x3a, 0x22, 0x43, 0x3a, 0x5c, 0x5c,
        0x77, 0x6f, 0x72, 0x6b, 0x22, 0x7d, 0x0a,
    ],
    &[
        0x28, 0xb5, 0x2f, 0xfd, 0x20, 0x6f, 0xcd, 0x02, 0x00, 0x42, 0xc5, 0x11, 0x18, 0x80, 0x39,
        0xe9, 0xe5, 0xbb, 0xda, 0xb6, 0x83, 0x88, 0xfa, 0x6e, 0x08, 0x85, 0x35, 0x10, 0x13, 0x04,
        0x5d, 0x01, 0x1d, 0x75, 0x54, 0xa1, 0xeb, 0x45, 0xe6, 0x20, 0x4f, 0x49, 0xd2, 0xc2, 0x56,
        0x23, 0xa2, 0x28, 0xb0, 0x11, 0x15, 0x51, 0x32, 0xae, 0x1c, 0x21, 0x86, 0x1a, 0x0b, 0x93,
        0xee, 0x3a, 0xa0, 0x6c, 0x1f, 0xc8, 0x8d, 0x7d, 0xf0, 0x1c, 0xd7, 0x25, 0x9c, 0xbb, 0x2a,
        0x80, 0xa5, 0xc7, 0x6c, 0xf7, 0x90, 0xc0, 0x9b, 0x05, 0x00, 0xaa, 0x51, 0x40, 0xa1, 0xb4,
        0xaa, 0x00, 0x39, 0x63, 0xc0, 0x05, 0x13, 0x0a,
    ],
    &[
        0x28, 0xb5, 0x2f, 0xfd, 0x20, 0xaa, 0xbd, 0x03, 0x00, 0x52, 0x48, 0x19, 0x1b, 0x90, 0x35,
        0xcd, 0x18, 0x64, 0xa1, 0xd5, 0x2f, 0x7e, 0x34, 0x11, 0x84, 0xad, 0x39, 0x11, 0x82, 0xec,
        0xba, 0x33, 0xd6, 0x6b, 0x38, 0x9a, 0xf3, 0xbc, 0x19, 0x04, 0x81, 0xd6, 0x9a, 0xa1, 0xdd,
        0x2e, 0x8a, 0x2e, 0x78, 0x29, 0x54, 0xa6, 0xb4, 0xcc, 0x85, 0x8d, 0xeb, 0xaa, 0x93, 0x10,
        0xc6, 0x21, 0xbf, 0x0f, 0x4d, 0x24, 0x29, 0xd5, 0x37, 0x42, 0x69, 0x04, 0x68, 0xad, 0x6b,
        0x23, 0xc7, 0x78, 0x2f, 0xc0, 0xd9, 0x02, 0xbe, 0xea, 0xac, 0x5a, 0xe6, 0x9b, 0x90, 0xc6,
        0x0b, 0x64, 0x86, 0x33, 0x3c, 0x37, 0x8b, 0xef, 0x63, 0xa4, 0xea, 0x49, 0x83, 0x55, 0xb9,
        0x8a, 0xa8, 0xe6, 0xf8, 0x49, 0x02, 0x76, 0x16, 0x05, 0x00, 0x76, 0x08, 0xea, 0xe7, 0x36,
        0xaa, 0x51, 0x1e, 0xa1, 0xb4, 0x16, 0x31, 0x9f,
    ],
    &[
        0x28, 0xb5, 0x2f, 0xfd, 0x20, 0x64, 0xa5, 0x02, 0x00, 0xd2, 0x04, 0x11, 0x18, 0x80, 0xb7,
        0x3a, 0xc4, 0x78, 0x22, 0x71, 0xc0, 0xf5, 0x24, 0x08, 0xd9, 0xce, 0x0b, 0x23, 0x06, 0xde,
        0x6d, 0xaf, 0xc0, 0x49, 0x8a, 0x54, 0x08, 0x43, 0x66, 0x22, 0xd9, 0xbb, 0x13, 0x4e, 0x08,
        0x50, 0x74, 0x3e, 0xb0, 0x5f, 0xfe, 0x62, 0x49, 0x4b, 0x0d, 0x55, 0xec, 0x41, 0x0e, 0x98,
        0xd6, 0xac, 0xfa, 0xb0, 0xe6, 0xe6, 0xb4, 0xf4, 0x89, 0xd0, 0x3c, 0x28, 0x12, 0x63, 0x2f,
        0xd6, 0x82, 0xc2, 0xfb, 0x02, 0x04, 0x00, 0x2a, 0x02, 0xb4, 0x06, 0x2c, 0x08, 0xcb, 0x6c,
        0xab, 0x3c, 0x01,
    ],
    &[
        0x28, 0xb5, 0x2f, 0xfd, 0x20, 0x26, 0x31, 0x01, 0x00, 0x7b, 0x22, 0x74, 0x79, 0x70, 0x65,
        0x22, 0x3a, 0x22, 0x74, 0x75, 0x72, 0x6e, 0x2f, 0x65, 0x6e, 0x64, 0x22, 0x2c, 0x22, 0x64,
        0x61, 0x74, 0x61, 0x22, 0x3a, 0x7b, 0x22, 0x74, 0x75, 0x72, 0x6e, 0x22, 0x3a, 0x31, 0x7d,
        0x7d, 0x0a,
    ],
];

fn joined(frames: &[&[u8]]) -> Vec<u8> {
    frames
        .iter()
        .flat_map(|frame| frame.iter().copied())
        .collect()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(Dsh.decode(bytes, usize::MAX).0).unwrap()
}

fn rows(text: impl AsRef<[u8]>) -> usize {
    text.as_ref()
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty())
        .count()
}

#[test]
fn frames_decode_in_order() {
    let decoded = text(&joined(ROWS));
    let lines: Vec<&str> = decoded.lines().collect();
    assert_eq!(lines.len(), ROWS.len());
    assert!(lines[0].contains("\"type\":\"session\""));
    assert!(lines[4].contains("\"turn/end\""));
}

#[test]
fn a_frame_being_written_is_left_for_the_next_read() {
    let whole = joined(ROWS);
    let mut bytes = whole.clone();
    bytes.truncate(bytes.len() - 10);
    let (decoded, consumed) = Dsh.decode(&bytes, usize::MAX);
    assert_eq!(rows(&decoded), ROWS.len() - 1);
    assert_eq!(consumed, whole.len() - ROWS[ROWS.len() - 1].len());

    // Once the frame is complete, reading on from there decodes just that row.
    let (rest, consumed) = Dsh.decode(&whole[consumed..], usize::MAX);
    assert_eq!(consumed, ROWS[ROWS.len() - 1].len());
    assert!(String::from_utf8(rest).unwrap().contains("\"turn/end\""));
}

#[test]
fn a_partial_magic_number_is_not_consumed() {
    let bytes = joined(&[ROWS[0], &ROWS[1][..2]]);
    let (_, consumed) = Dsh.decode(&bytes, usize::MAX);
    assert_eq!(consumed, ROWS[0].len());
}

#[test]
fn a_corrupt_frame_is_skipped() {
    let mut corrupt = ROWS[1].to_vec();
    // An unknown block type makes the frame undecodable.
    corrupt[6] = 0xff;
    corrupt[7] = 0xff;
    corrupt[8] = 0xff;
    let bytes = joined(&[ROWS[0], &corrupt, ROWS[2], ROWS[3], ROWS[4]]);
    let (decoded, consumed) = Dsh.decode(&bytes, usize::MAX);
    assert_eq!(consumed, bytes.len());
    assert_eq!(rows(&decoded), ROWS.len() - 1);
}

#[test]
fn decoding_resumes_at_the_next_frame() {
    // A preview reading the tail of a long log starts inside a frame; the
    // frames after it decode as usual.
    let bytes = joined(ROWS);
    let decoded = text(&bytes[ROWS[0].len() + 20..]);
    assert_eq!(rows(&decoded), ROWS.len() - 2);
    assert!(decoded.contains("\"tool/result\""));
    assert!(decoded.contains("\"turn/end\""));
}

#[test]
fn decoding_stops_at_the_frame_that_reaches_the_limit() {
    let (decoded, consumed) = Dsh.decode(&joined(ROWS), 40);
    assert_eq!(rows(&decoded), 1);
    assert_eq!(consumed, ROWS[0].len());
}

#[test]
fn the_session_record_identifies_its_directory() {
    let row = json!({"type":"session","version":4,"id":"session-abc"});
    assert!(Dsh.is_session_record(&row, "session-abc"));
    assert!(!Dsh.is_session_record(&row, "abc"));
}

#[test]
fn a_log_belongs_to_the_session_named_by_its_directory() {
    let path = Path::new("/sessions/--C--work--/session-abc/session.v4.jsonl.zstd");
    assert!(Dsh.is_session_log(path, "session-abc"));
    // Every session writes the same file name, so the directory decides.
    assert!(!Dsh.is_session_log(path, "session-def"));
}

#[test]
fn a_user_message_opens_a_turn() {
    let record = Dsh
        .parse(&json!({
            "type":"user/message",
            "data":{"id":"m1","source":{"kind":"user"},"content":[{"type":"text","text":"hello"}]}
        }))
        .unwrap();
    assert!(record.starts_turn);
    assert!(!record.ends_turn);
    assert_eq!(record.entries, vec![prompt("hello".into())]);
}

#[test]
fn injected_messages_are_context() {
    for kind in ["runtime-context", "skill-catalog", "agent-instructions"] {
        let record = Dsh
            .parse(&json!({
                "type":"user/message",
                "data":{"source":{"kind":kind},"content":[{"type":"text","text":"env"}]}
            }))
            .unwrap();
        assert!(!record.starts_turn, "{kind}");
        assert_eq!(record.entries, vec![context("env".into())], "{kind}");
    }
}

#[test]
fn assistant_messages_carry_text_and_tool_calls() {
    let record = Dsh
        .parse(&json!({
            "type":"assistant/message",
            "data":{"message":{"id":"m2","content":[
                {"type":"reasoning","text":"private"},
                {"type":"text","text":"Running it"},
                {"type":"tool-call","name":"pwsh","arguments":"{\"command\":\"ls\"}"}
            ]}}
        }))
        .unwrap();
    assert_eq!(
        record.entries,
        vec![
            Entry::Thinking {
                text: "private".into()
            },
            said("Running it".into()),
            Entry::ToolCall {
                name: "pwsh".into(),
                input: "{\"command\":\"ls\"}".into(),
            },
        ]
    );
}

#[test]
fn tool_calls_are_not_rendered_twice() {
    // The `tool/call` row repeats the call the assistant message already has.
    assert!(
        Dsh.parse(&json!({
            "type":"tool/call",
            "data":{"name":"pwsh","arguments":"{}"}
        }))
        .is_none()
    );
    let record = Dsh
        .parse(&json!({
            "type":"tool/result",
            "data":{"message":{"id":"m3","content":[{"type":"text","text":"output"}]}}
        }))
        .unwrap();
    assert_eq!(
        record.entries,
        vec![Entry::ToolResult {
            text: "output".into()
        }]
    );
}

#[test]
fn the_turn_end_marker_closes_a_turn() {
    let record = Dsh
        .parse(&json!({"type":"turn/end","data":{"turn":1}}))
        .unwrap();
    assert!(record.ends_turn);
    assert!(record.entries.is_empty());
}

#[test]
fn records_without_a_conversation_part_are_ignored() {
    for row in [
        json!({"type":"step/start","data":{"turn":1,"step":1}}),
        json!({"type":"system/message","data":{"message":{"content":[]}}}),
        json!({"type":"agent/inbox/spliced","data":{}}),
    ] {
        assert!(Dsh.parse(&row).is_none(), "{row}");
    }
}

#[test]
fn a_message_id_identifies_its_revisions() {
    assert_eq!(
        Dsh.record_id(&json!({"type":"assistant/message","data":{"message":{"id":"m2"}}})),
        Some("m2")
    );
    assert_eq!(
        Dsh.record_id(&json!({"type":"user/message","data":{"id":"m1"}})),
        Some("m1")
    );
    assert_eq!(Dsh.record_id(&json!({"type":"turn/end","data":{}})), None);
}

#[test]
fn install_writes_the_plugin_once_and_uninstall_removes_it() {
    let dir = tempdir().unwrap();
    let ctx = AppContext::for_test(dir.path(), &dir.path().join("bin/agent-berth"));
    std::fs::create_dir_all(&ctx.dsh_home).unwrap();

    let dest = Dsh.install(&ctx).unwrap();
    assert_eq!(dest, ctx.dsh_home.join("agent-berth/plugin.mjs"));
    let plugin = std::fs::read_to_string(&dest).unwrap();
    assert!(plugin.contains("notify\", \"--provider\", \"dsh\""));
    assert!(!plugin.contains("__AGENT_BERTH_BIN__"));

    // Installing again replaces the block instead of appending a second one.
    Dsh.install(&ctx).unwrap();
    let patch = std::fs::read_to_string(ctx.dsh_home.join("cordis.patch.yml")).unwrap();
    assert_eq!(patch.matches(BEGIN).count(), 1);
    assert!(patch.contains("name: ./agent-berth/plugin.mjs"));
    assert!(!Dsh.outdated(&ctx));

    Dsh.uninstall(&ctx).unwrap();
    assert!(!dest.exists());
    assert!(!ctx.dsh_home.join("cordis.patch.yml").exists());
}

#[test]
fn the_patch_row_leaves_the_rest_of_the_layer_alone() {
    let dir = tempdir().unwrap();
    let ctx = AppContext::for_test(dir.path(), &dir.path().join("bin/agent-berth"));
    std::fs::create_dir_all(&ctx.dsh_home).unwrap();
    let patch = ctx.dsh_home.join("cordis.patch.yml");
    let user_row = "- id: llm-deepseek\n  disabled: true\n";
    std::fs::write(&patch, format!("# mine\n{user_row}")).unwrap();

    Dsh.install(&ctx).unwrap();
    let text = std::fs::read_to_string(&patch).unwrap();
    assert!(text.starts_with("# mine\n"));
    assert!(text.contains(user_row));
    assert!(text.trim_end().ends_with(END));

    Dsh.uninstall(&ctx).unwrap();
    assert_eq!(
        std::fs::read_to_string(&patch).unwrap(),
        format!("# mine\n{user_row}")
    );
}

#[test]
fn a_block_missing_its_end_marker_keeps_the_rows_after_it() {
    let user_row = "- id: llm-deepseek\n  disabled: true\n";
    let block = patch_block();
    let unterminated = block.strip_suffix(&format!("{END}\n")).unwrap();
    assert_eq!(
        without_patch_row(&format!("{unterminated}{user_row}")),
        user_row
    );
}

/// A hub that hosts every session and reports what it was asked to do.
#[derive(Debug)]
struct Pane;

impl Hub for Pane {
    fn hosts(&self, _session: &ListedSession) -> bool {
        true
    }

    fn focus(&self, _ctx: &AppContext, _session: &ListedSession) -> Result<()> {
        anyhow::bail!("pane focused")
    }

    fn send(&self, _session: &ListedSession, _message: &str) -> Result<()> {
        anyhow::bail!("pane typed into")
    }

    fn transcript(&self, _session: &ListedSession) -> Result<Conversation> {
        anyhow::bail!("no transcript")
    }

    fn preview(&self, _ctx: &AppContext, _session: &ListedSession) -> Option<Preview> {
        None
    }
}

fn listed(source: &str) -> ListedSession {
    serde_json::from_value(json!({
        "provider":"dsh", "session_id":"s", "status":"idle", "source":source,
        "cwd":null, "cmdline":[], "pid":null, "created_ms":1,
        "last_report_ms":2, "kind":"plugin"
    }))
    .unwrap()
}

#[test]
fn a_terminal_session_is_reached_through_its_pane() {
    let dir = tempdir().unwrap();
    let ctx = AppContext::for_test(dir.path(), &dir.path().join("bin/agent-berth"));
    let session = listed("cli");
    let focus = Dsh.focus(&ctx, &Pane, &session).unwrap_err();
    assert_eq!(focus.to_string(), "pane focused");
    let send = Dsh.send(&Pane, &session, "hi").unwrap_err();
    assert_eq!(send.to_string(), "pane typed into");
}

#[test]
fn a_desktop_session_takes_no_message() {
    let error = Dsh.send(&Pane, &listed("desktop"), "hi").unwrap_err();
    assert!(
        error.to_string().contains("DeepSeek Harness app"),
        "{error}"
    );
}

#[test]
fn a_stale_plugin_is_outdated() {
    let dir = tempdir().unwrap();
    let ctx = AppContext::for_test(dir.path(), &dir.path().join("bin/agent-berth"));
    std::fs::create_dir_all(&ctx.dsh_home).unwrap();
    Dsh.install(&ctx).unwrap();
    assert!(!Dsh.outdated(&ctx));

    std::fs::write(Dsh.hook_path(&ctx), "export const name = \"old\"").unwrap();
    assert!(Dsh.outdated(&ctx));

    // A plugin from this build, but with its patch row dropped, is outdated too.
    Dsh.install(&ctx).unwrap();
    std::fs::write(ctx.dsh_home.join("cordis.patch.yml"), "[]\n").unwrap();
    assert!(Dsh.outdated(&ctx));
}

#[test]
fn archived_sessions_are_quiet_until_the_file_says_otherwise() {
    let dir = tempdir().unwrap();
    let ctx = AppContext::for_test(dir.path(), &dir.path().join("bin/agent-berth"));
    assert!(Dsh.quiet_ids(&ctx).is_empty());

    let store = ctx.dsh_home.join("storages");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(
        store.join("workspace.json"),
        r#"{"global":{"archivedSessionIds":["session-old","session-new"]}}"#,
    )
    .unwrap();
    assert_eq!(
        Dsh.quiet_ids(&ctx),
        ["session-new".to_string(), "session-old".to_string()]
            .into_iter()
            .collect()
    );

    // A torn write keeps the last set instead of flashing archived rows back.
    std::fs::write(store.join("workspace.json"), "{").unwrap();
    assert_eq!(Dsh.quiet_ids(&ctx).len(), 2);

    std::fs::remove_file(store.join("workspace.json")).unwrap();
    assert!(Dsh.quiet_ids(&ctx).is_empty());
}

#[test]
fn the_log_lives_under_the_home_sessions_directory() {
    let dir = tempdir().unwrap();
    let ctx = AppContext::for_test(dir.path(), &dir.path().join("bin/agent-berth"));
    assert_eq!(Dsh.root(&ctx), ctx.dsh_home.join("sessions"));
}
