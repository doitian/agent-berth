use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use serde_json::Value;

use super::{
    Focus, Integration, Messaging, Provider, Transcript, TranscriptLog, focus_or_desktop,
    log_preview, log_transcript, open_desktop_url, write_text,
};
use crate::hubs::{Hub, Preview};
use crate::paths::Context;
use crate::status::Source;
use crate::store::ListedSession;
use crate::transcript::{Conversation, Record, content, context, prompt, said, tool_result};

/// The log a DeepSeek Harness profile appends inside each session directory,
/// one zstd frame per write.
const SESSION_LOG: &str = "session.v4.jsonl.zstd";
/// The Cordis plugin reporting session status, relative to `DSH_HOME`. The
/// `.mjs` extension keeps it a module: the Harness home has no `package.json`
/// to mark `.js` files as ESM.
const PLUGIN: &str = "agent-berth/plugin.mjs";
/// The machine-wide patch layer every profile composes, relative to `DSH_HOME`.
const PATCH: &str = "cordis.patch.yml";
const BEGIN: &str = "# >>> agent-berth";
const END: &str = "# <<< agent-berth";
/// Zstandard frame header, the only bytes a frame starts with.
const MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

pub struct Dsh;

impl Provider for Dsh {
    fn name(&self) -> &'static str {
        "dsh"
    }

    fn logo(&self) -> &'static str {
        "❖"
    }

    /// Sessions are booted by the Harness app or by a profile the launcher
    /// starts (`dsh tui`, `dsh web`, `dsh headless`), which owns the
    /// conversation; agent-berth only lists them. Desktop sessions are never
    /// resumed from here, and the profile that booted one is not recorded in
    /// its log, so a terminal session is reopened through the generic profile,
    /// which adopts the stored session by id.
    fn resume_cmd(&self, session_id: &str) -> Vec<String> {
        vec![
            "dsh".into(),
            "tui".into(),
            "--resume".into(),
            session_id.into(),
        ]
    }

    fn session_env(&self) -> Option<&'static str> {
        Some("DSH_SESSION_ID")
    }

    fn log(&self) -> Option<&dyn TranscriptLog> {
        Some(self)
    }

    fn quiet_ids(&self, ctx: &Context) -> BTreeSet<String> {
        archived_ids(ctx)
    }
}

impl Integration for Dsh {
    fn install(&self, ctx: &Context) -> Result<PathBuf> {
        let dest = self.hook_path(ctx);
        write_text(&dest, &plugin_source(ctx))?;
        let patch = ctx.dsh_home.join(PATCH);
        let current = std::fs::read_to_string(&patch).unwrap_or_default();
        let next = with_patch_row(&current);
        if next != current {
            write_text(&patch, &next)?;
        }
        Ok(dest)
    }

    fn uninstall(&self, ctx: &Context) -> Result<()> {
        let dest = self.hook_path(ctx);
        if dest.exists() {
            std::fs::remove_file(&dest)?;
        }
        if let Some(dir) = dest.parent() {
            // Only removes the directory when the plugin was its last file.
            let _ = std::fs::remove_dir(dir);
        }
        let patch = ctx.dsh_home.join(PATCH);
        if let Ok(current) = std::fs::read_to_string(&patch) {
            let next = without_patch_row(&current);
            if next != current {
                if next.trim().is_empty() {
                    std::fs::remove_file(&patch)?;
                } else {
                    write_text(&patch, &next)?;
                }
            }
        }
        Ok(())
    }

    fn hook_path(&self, ctx: &Context) -> PathBuf {
        ctx.dsh_home.join(PLUGIN)
    }

    fn outdated(&self, ctx: &Context) -> bool {
        let patch = std::fs::read_to_string(ctx.dsh_home.join(PATCH)).unwrap_or_default();
        std::fs::read_to_string(self.hook_path(ctx)).is_ok_and(|text| text != plugin_source(ctx))
            || !has_patch_row(&patch)
    }

    fn config_exists(&self, ctx: &Context) -> bool {
        ctx.dsh_home.is_dir()
    }
}

impl Focus for Dsh {
    /// A session the app booted lives in its window, which no tmux pane hosts,
    /// so focusing it raises the app through the `dsh://` link it registers. A
    /// terminal session, such as one `resume` reopened, is focused in its pane.
    fn focus(&self, ctx: &Context, hub: &dyn Hub, session: &ListedSession) -> Result<()> {
        focus_or_desktop(ctx, hub, session, || open_desktop_url("dsh://open"))
    }
}

impl Messaging for Dsh {
    /// The app owns a desktop session's conversation and offers no command
    /// that takes a message, so it can only be continued in the app. A terminal
    /// session is typed into like any other.
    fn send(&self, hub: &dyn Hub, session: &ListedSession, message: &str) -> Result<()> {
        if session.source != Source::Desktop {
            return hub.send(session, message);
        }
        anyhow::bail!(
            "{} {} runs in the DeepSeek Harness app, which takes no message from here",
            session.provider,
            session.session_id
        )
    }
}

impl Transcript for Dsh {
    fn transcript(
        &self,
        ctx: &Context,
        hub: &dyn Hub,
        session: &ListedSession,
    ) -> Result<Conversation> {
        log_transcript(self, ctx, hub, session)
    }

    fn preview(&self, ctx: &Context, hub: &dyn Hub, session: &ListedSession) -> Option<Preview> {
        log_preview(self, ctx, hub, session)
    }
}

impl TranscriptLog for Dsh {
    fn root(&self, ctx: &Context) -> PathBuf {
        ctx.dsh_home.join("sessions")
    }

    /// The log's name is fixed, so the session is identified by the directory
    /// holding it: `sessions/<escaped-cwd>/<session-id>/session.v4.jsonl.zstd`.
    fn is_session_file(&self, file_name: &str, _session_id: &str) -> bool {
        file_name == SESSION_LOG
    }

    fn is_session_log(&self, path: &Path, session_id: &str) -> bool {
        path.file_name().is_some_and(|name| name == SESSION_LOG)
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == session_id)
    }

    fn is_session_record(&self, row: &Value, session_id: &str) -> bool {
        row["type"] == "session" && row["id"].as_str() == Some(session_id)
    }

    fn parse(&self, row: &Value) -> Option<Record> {
        let data = &row["data"];
        match row["type"].as_str()? {
            "user/message" => {
                // Records the harness injects, such as instructions, a
                // scheduled wake-up, or a subagent's report, are context for the
                // agent, not its prompt.
                let from_user = data["source"]["kind"] == "user";
                Some(Record {
                    entries: content(&data["content"], if from_user { prompt } else { context }),
                    starts_turn: from_user,
                    ends_turn: false,
                })
            }
            "assistant/message" => Some(Record {
                entries: content(&data["message"]["content"], said),
                starts_turn: false,
                ends_turn: false,
            }),
            "tool/result" => Some(Record {
                entries: tool_result(&data["message"]["content"]),
                ..Record::default()
            }),
            "turn/end" => Some(Record {
                ends_turn: true,
                ..Record::default()
            }),
            // `tool/call` repeats the tool call the assistant message already
            // carries, so rendering it too would show every call twice.
            _ => None,
        }
    }

    /// A message's own id, so a row that rewrites an earlier one replaces it
    /// instead of appending a copy.
    fn record_id<'a>(&self, row: &'a Value) -> Option<&'a str> {
        let data = &row["data"];
        data["message"]["id"]
            .as_str()
            .or_else(|| data["id"].as_str())
    }

    fn compressed(&self) -> bool {
        true
    }

    fn decode(&self, bytes: &[u8], limit: usize) -> (Vec<u8>, usize) {
        decode_frames(bytes, limit)
    }
}

/// Decodes the frames a session log is appended in until at least `limit`
/// bytes of text are produced, returning the text and how many bytes the
/// decoded frames span. A last frame still being written is left undecoded.
fn decode_frames(bytes: &[u8], limit: usize) -> (Vec<u8>, usize) {
    let mut text = Vec::new();
    let mut done = 0;
    while done < bytes.len() && text.len() < limit {
        // Reading from the end of a log lands mid-frame; the first complete
        // frame starts at the next magic number.
        let Some(at) = find_frame(&bytes[done..]) else {
            // Keep a tail that may be a magic number still being written.
            done = done.max(bytes.len().saturating_sub(MAGIC.len() - 1));
            break;
        };
        let start = done + at;
        let frame = &bytes[start..];
        if let Some((decoded, len)) = decode_frame(frame) {
            text.extend_from_slice(&decoded);
            done = start + len;
            continue;
        }
        // A frame that another one follows is corrupt and skipped; the last
        // one is still being written.
        match find_frame(&frame[MAGIC.len()..]) {
            Some(next) => done = start + MAGIC.len() + next,
            None => {
                done = start;
                break;
            }
        }
    }
    (text, done)
}

fn find_frame(bytes: &[u8]) -> Option<usize> {
    bytes.windows(MAGIC.len()).position(|word| word == MAGIC)
}

/// One whole frame's text and length, or `None` when it is truncated or
/// corrupt, so a partial frame never contributes half a row.
fn decode_frame(frame: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(frame).ok()?;
    let mut text = Vec::new();
    decoder.read_to_end(&mut text).ok()?;
    Some((text, frame.len() - decoder.into_inner().len()))
}

struct ArchiveCache {
    path: PathBuf,
    ids: BTreeSet<String>,
}

/// Archived session ids from the workspace store every profile shares.
/// A torn write keeps the last set read whole, so a heartbeat does not flash
/// every archived session back into the list.
fn archived_ids(ctx: &Context) -> BTreeSet<String> {
    static CACHE: Mutex<Option<ArchiveCache>> = Mutex::new(None);
    let path = ctx.dsh_home.join("storages").join("workspace.json");
    let remembered = || {
        CACHE
            .lock()
            .ok()
            .and_then(|cache| {
                cache
                    .as_ref()
                    .filter(|cache| cache.path == path)
                    .map(|cache| cache.ids.clone())
            })
            .unwrap_or_default()
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Ok(mut cache) = CACHE.lock()
                && cache.as_ref().is_some_and(|cache| cache.path == path)
            {
                *cache = None;
            }
            return BTreeSet::new();
        }
        Err(_) => return remembered(),
    };
    match parse_archived(&text) {
        Some(ids) => {
            if let Ok(mut cache) = CACHE.lock() {
                *cache = Some(ArchiveCache {
                    path: path.clone(),
                    ids: ids.clone(),
                });
            }
            ids
        }
        None => remembered(),
    }
}

/// `None` when the file is not valid JSON, so a torn write can keep the last
/// set. A readable file with no archive list is an empty set.
fn parse_archived(text: &str) -> Option<BTreeSet<String>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let Some(ids) = value
        .pointer("/global/archivedSessionIds")
        .and_then(serde_json::Value::as_array)
    else {
        return Some(BTreeSet::new());
    };
    Some(
        ids.iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect(),
    )
}

fn plugin_source(ctx: &Context) -> String {
    include_str!("hooks/dsh.mjs").replace(
        "__AGENT_BERTH_BIN__",
        &serde_json::to_string(&ctx.berth_bin.display().to_string()).unwrap(),
    )
}

/// The patch row inserting the plugin, anchored beside the patch file so every
/// profile composing this layer resolves it.
fn patch_block() -> String {
    let mut block = String::new();
    block.push_str(BEGIN);
    block.push('\n');
    block.push_str("# Status reporting for DeepSeek Harness sessions, installed by\n");
    block.push_str("# `agent-berth setup` and removed by `agent-berth teardown`.\n");
    block.push_str("- insert:\n");
    block.push_str("    - id: agent-berth\n");
    block.push_str(&format!("      name: ./{PLUGIN}\n"));
    block.push_str(END);
    block.push('\n');
    block
}

fn has_patch_row(text: &str) -> bool {
    text.lines().any(|line| line.trim_end() == BEGIN)
}

/// `text` with this build's row, replacing one an older build left behind.
fn with_patch_row(text: &str) -> String {
    let mut next = without_patch_row(text);
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(&patch_block());
    next
}

/// `text` without this build's row, leaving the rest of the user's layer as it
/// was.
fn without_patch_row(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed == BEGIN {
            inside = true;
        } else if inside {
            inside = trimmed != END;
            // A block whose end marker was deleted stops at the next top-level
            // row, which is the user's own.
            if inside && is_user_row(trimmed) {
                inside = false;
                out.push_str(line);
            }
        } else {
            out.push_str(line);
        }
    }
    if strip_comments(&out) == "[]" {
        return String::new();
    }
    out
}

fn is_user_row(line: &str) -> bool {
    !line.is_empty() && !line.starts_with([' ', '#']) && line != "- insert:"
}

/// The layer with comment and blank lines removed, to tell an empty entry list
/// from one that holds rows.
fn strip_comments(text: &str) -> String {
    let kept: String = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    kept.trim().to_string()
}

#[cfg(test)]
#[path = "dsh_tests.rs"]
mod tests;
