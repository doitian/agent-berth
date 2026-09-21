//! Bounded, incremental readers for local desktop conversation logs.
use std::collections::VecDeque;
use std::fs::{File, Metadata};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

const READ_LIMIT: u64 = 1024 * 1024;
const RECORD_LIMIT: usize = 256 * 1024;
const ENTRY_LIMIT: usize = 48;
const TEXT_LIMIT: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Source {
    pub provider: String,
    pub session_id: String,
    pub path: Option<String>,
    pub root: Option<PathBuf>,
}

#[derive(Default)]
pub(crate) struct Reader {
    offset: u64,
    pending: Vec<u8>,
    skipping: bool,
    entries: VecDeque<(u64, String)>,
    checkpoint: Vec<u8>,
    modified: Option<SystemTime>,
    identity: Option<FileIdentity>,
    omitted: bool,
    resolved: Option<PathBuf>,
}

#[derive(PartialEq, Eq)]
struct FileIdentity {
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl FileIdentity {
    fn new(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            created: metadata.created().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }
}

impl Reader {
    pub fn preview(&mut self, source: &Source) -> String {
        let path = source
            .path
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| self.resolved.clone().or_else(|| locate(source)));
        let Some(path) = path else {
            return "Transcript unavailable: waiting for a session hook with a transcript path."
                .into();
        };
        match self.read(&path, &source.provider) {
            Ok(()) => {
                self.resolved = Some(path);
                self.render()
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.resolved = None;
                "Transcript unavailable: the file is missing. Retrying…".into()
            }
            Err(error) => format!("Unable to read transcript ({}). Retrying…", error.kind()),
        }
    }

    fn read(&mut self, path: &Path, provider: &str) -> io::Result<()> {
        // Check before opening so a stale/misconfigured hook cannot point us at
        // a pipe or device that blocks a worker indefinitely.
        if !path.metadata()?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file"));
        }
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        let identity = FileIdentity::new(&metadata);
        let modified = metadata.modified().ok();
        let replaced = self.identity.as_ref().is_some_and(|old| old != &identity);
        let rewritten = self.offset == metadata.len() && self.modified != modified;
        let mut reset = replaced || metadata.len() < self.offset || rewritten;
        if !reset && !self.checkpoint.is_empty() {
            file.seek(SeekFrom::Start(self.offset - self.checkpoint.len() as u64))?;
            let mut check = vec![0; self.checkpoint.len()];
            file.read_exact(&mut check)?;
            reset = check != self.checkpoint;
        }
        if reset {
            *self = Self::default();
        }
        self.identity = Some(identity);
        self.modified = modified;
        // On initial selection, or if the producer outruns the reader, stay
        // near the live end without reading an unbounded history.
        if metadata.len().saturating_sub(self.offset) > READ_LIMIT {
            self.offset = metadata.len() - READ_LIMIT;
            self.pending.clear();
            self.skipping = true;
            self.omitted = true;
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        (&mut file).take(READ_LIMIT).read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;
        for chunk in bytes.split_inclusive(|byte| *byte == b'\n') {
            let complete = chunk.last() == Some(&b'\n');
            if !self.skipping {
                if self.pending.len() + chunk.len() > RECORD_LIMIT {
                    self.pending.clear();
                    self.skipping = true;
                    self.omitted = true;
                } else {
                    self.pending.extend_from_slice(chunk);
                }
            }
            if complete {
                if !self.skipping {
                    self.record(provider);
                }
                self.pending.clear();
                self.skipping = false;
            }
        }
        let count = self.offset.min(256) as usize;
        file.seek(SeekFrom::Start(self.offset - count as u64))?;
        self.checkpoint.resize(count, 0);
        file.read_exact(&mut self.checkpoint)?;
        Ok(())
    }

    fn record(&mut self, provider: &str) {
        let Ok(row) = serde_json::from_slice::<Value>(&self.pending) else {
            self.omitted = true;
            return;
        };
        let Some(text) = parse(provider, &row) else {
            return;
        };
        let mut hash = DefaultHasher::new();
        // Claude UUIDs identify records, including revisions. Codex event_msg
        // mirrors are ignored by the parser; timestamped response rows identify
        // distinct messages even when their text happens to be identical.
        if provider == "claude" && row["uuid"].is_string() {
            row["uuid"].as_str().hash(&mut hash);
        } else {
            row.to_string().hash(&mut hash);
        }
        let key = hash.finish();
        if let Some((_, existing)) = self.entries.iter_mut().find(|(id, _)| *id == key) {
            *existing = text;
        } else {
            self.entries.push_back((key, text));
            if self.entries.len() > ENTRY_LIMIT {
                self.entries.pop_front();
                self.omitted = true;
            }
        }
    }

    fn render(&self) -> String {
        let mut text = if self.omitted {
            "[Showing recent content; older, oversized, or malformed records omitted]\n\n".into()
        } else {
            String::new()
        };
        for (_, entry) in &self.entries {
            text.push_str(entry);
            text.push_str("\n\n");
        }
        if self.entries.is_empty() {
            text.push_str("No conversation messages yet.");
        }
        text
    }
}

/// Recover paths for already-running sessions. Search only the configured local
/// transcript root, never symlinked directories or guessed parent transcripts.
fn locate(source: &Source) -> Option<PathBuf> {
    let id = &source.session_id;
    if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return None;
    }
    let mut dirs = vec![(source.root.clone()?, 0)];
    let mut visited = 0;
    while let Some((dir, depth)) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > 10_000 {
                return None;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && depth < 4 {
                dirs.push((entry.path(), depth + 1));
            } else if kind.is_file() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let matches = match source.provider.as_str() {
                    "claude" => name == format!("{id}.jsonl"),
                    "codex" => {
                        name.starts_with("rollout-") && name.ends_with(&format!("-{id}.jsonl"))
                    }
                    _ => false,
                };
                if matches && belongs_to(&entry.path(), source) {
                    return Some(entry.path());
                }
            }
        }
    }
    None
}

fn belongs_to(path: &Path, source: &Source) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut bytes = Vec::new();
    if file
        .take(RECORD_LIMIT as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return false;
    }
    bytes
        .split(|c| *c == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .any(|row| match source.provider.as_str() {
            "claude" => row["sessionId"].as_str() == Some(source.session_id.as_str()),
            "codex" => {
                row["type"] == "session_meta"
                    && row["payload"]["id"]
                        .as_str()
                        .or_else(|| row["payload"]["session_id"].as_str())
                        == Some(source.session_id.as_str())
            }
            _ => false,
        })
}

fn parse(provider: &str, row: &Value) -> Option<String> {
    let mut output = String::new();
    match provider {
        "claude" if matches!(row["type"].as_str(), Some("user" | "assistant")) => {
            let role = row["type"].as_str()?;
            content(&mut output, role, &row["message"]["content"]);
        }
        "codex" if row["type"] == "response_item" => {
            let item = &row["payload"];
            match item["type"].as_str()? {
                "message" => {
                    let role = item["role"].as_str()?;
                    if !matches!(role, "user" | "assistant") {
                        return None;
                    }
                    content(&mut output, role, &item["content"]);
                }
                "function_call" | "custom_tool_call" => {
                    append(&mut output, "Tool", item["name"].as_str().unwrap_or("call"));
                    append(
                        &mut output,
                        "Input",
                        item["arguments"]
                            .as_str()
                            .or_else(|| item["input"].as_str())
                            .unwrap_or(""),
                    );
                }
                "function_call_output" | "custom_tool_call_output" => {
                    tool_result(&mut output, &item["output"]);
                }
                _ => return None,
            }
        }
        _ => return None,
    }
    (!output.is_empty()).then_some(output)
}

fn content(output: &mut String, role: &str, value: &Value) {
    let label = if role == "user" { "You" } else { "Assistant" };
    if let Some(text) = value.as_str() {
        append(output, label, text);
    } else if let Some(blocks) = value.as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("text" | "input_text" | "output_text") => {
                    append(output, label, block["text"].as_str().unwrap_or(""));
                }
                Some("tool_use") => {
                    append(output, "Tool", block["name"].as_str().unwrap_or("call"));
                    append(output, "Input", &block["input"].to_string());
                }
                Some("tool_result") => tool_result(output, &block["content"]),
                Some("image" | "input_image") => append(output, label, "[image]"),
                _ => {}
            }
        }
    }
}

fn tool_result(output: &mut String, value: &Value) {
    if let Some(text) = value.as_str() {
        append(output, "Result", text);
    } else if let Some(blocks) = value.as_array() {
        for block in blocks {
            if let Some(text) = block["text"].as_str() {
                append(output, "Result", text);
            }
        }
    }
}

fn append(output: &mut String, label: &str, text: &str) {
    // Plain text only: no transcript-controlled terminal escapes or C1 controls.
    let remaining = TEXT_LIMIT.saturating_sub(output.chars().count());
    if remaining <= label.len() + 4 || text.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(label);
    output.push_str(": ");
    let mut chars = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'));
    output.extend(chars.by_ref().take(remaining - label.len() - 4));
    if chars.next().is_some() {
        output.push('…');
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
