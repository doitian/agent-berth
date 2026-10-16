//! Readers for local agent conversation logs: a bounded incremental reader for
//! live previews and a one-shot reader that renders a whole transcript for CLI
//! output.
use std::collections::{HashMap, VecDeque};
use std::fs::{File, Metadata};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::providers::TranscriptLog;

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

fn log(provider: &str) -> Option<&'static dyn TranscriptLog> {
    crate::providers::lookup(provider)?.log()
}

/// A live conversation the TUI previews.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Feed {
    Log(Source),
    Paseo { agent_id: String, home: PathBuf },
}

/// Whether `Reader` can parse and locate this provider's conversation logs.
pub(crate) fn supports(provider: &str) -> bool {
    log(provider).is_some()
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
    paseo: Option<crate::hubs::paseo::Stream>,
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
    pub fn preview(&mut self, feed: &Feed) -> String {
        match feed {
            Feed::Log(source) => self.preview_log(source),
            Feed::Paseo { agent_id, home } => self
                .paseo
                .get_or_insert_with(|| crate::hubs::paseo::Stream::new(agent_id, home))
                .preview(),
        }
    }

    fn preview_log(&mut self, source: &Source) -> String {
        let Some(log) = log(&source.provider) else {
            return format!("{} sessions have no local transcript", source.provider);
        };
        let path = source
            .path
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| self.resolved.clone().or_else(|| locate(source)));
        let Some(path) = path else {
            return "Transcript unavailable: waiting for a session hook with a transcript path."
                .into();
        };
        match self.read(&path, log) {
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

    fn read(&mut self, path: &Path, log: &dyn TranscriptLog) -> io::Result<()> {
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
                    self.record(log);
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

    fn record(&mut self, log: &dyn TranscriptLog) {
        let Ok(row) = serde_json::from_slice::<Value>(&self.pending) else {
            self.omitted = true;
            return;
        };
        let Some(text) = parse(log, &row) else {
            return;
        };
        let key = record_key(log, &row);
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

/// Stable key for a transcript record, used to replace revisions of the same
/// message. Records without an ID are keyed by their content: Codex event_msg
/// mirrors are ignored by the parser, and timestamped response rows identify
/// distinct messages even when their text happens to be identical.
fn record_key(log: &dyn TranscriptLog, row: &Value) -> u64 {
    let mut hash = DefaultHasher::new();
    match log.record_id(row) {
        Some(id) => id.hash(&mut hash),
        None => row.to_string().hash(&mut hash),
    }
    hash.finish()
}

/// Render a whole local transcript, oldest entry first, for one-shot CLI
/// output. Unlike `Reader`, it reads the entire file, keeps every entry, and
/// does not truncate message text.
pub(crate) fn full(source: &Source) -> Result<String> {
    let log = log(&source.provider)
        .with_context(|| format!("{} sessions have no local transcript", source.provider))?;
    let path = match source.path.as_ref().map(PathBuf::from) {
        Some(path) => path,
        None => locate(source).with_context(|| {
            format!(
                "no transcript file found for {} {}",
                source.provider, source.session_id
            )
        })?,
    };
    let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
    let mut order: Vec<u64> = Vec::new();
    let mut texts: HashMap<u64, String> = HashMap::new();
    for line in BufReader::new(file).lines() {
        let line = line.with_context(|| format!("read {}", path.display()))?;
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(text) = parse_with_limit(log, &row, usize::MAX) else {
            continue;
        };
        let key = record_key(log, &row);
        if texts.insert(key, text).is_none() {
            order.push(key);
        }
    }
    let entries: Vec<&str> = order
        .iter()
        .filter_map(|key| texts.get(key).map(String::as_str))
        .collect();
    Ok(entries.join("\n\n"))
}

/// Recover paths for already-running sessions. Search only the configured local
/// transcript root, never symlinked directories or guessed parent transcripts.
fn locate(source: &Source) -> Option<PathBuf> {
    let log = log(&source.provider)?;
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
                if log.is_session_file(&name, id) && belongs_to(log, &entry.path(), id) {
                    return Some(entry.path());
                }
            }
        }
    }
    None
}

fn belongs_to(log: &dyn TranscriptLog, path: &Path, session_id: &str) -> bool {
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
        .any(|row| log.is_session_record(&row, session_id))
}

fn parse(log: &dyn TranscriptLog, row: &Value) -> Option<String> {
    parse_with_limit(log, row, TEXT_LIMIT)
}

fn parse_with_limit(log: &dyn TranscriptLog, row: &Value, limit: usize) -> Option<String> {
    log.parse(row, limit).filter(|output| !output.is_empty())
}

pub(crate) fn content(output: &mut String, role: &str, value: &Value, limit: usize) {
    let label = if role == "user" { "You" } else { "Assistant" };
    if let Some(text) = value.as_str() {
        append(output, label, text, limit);
    } else if let Some(blocks) = value.as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("text" | "input_text" | "output_text") => {
                    append(output, label, block["text"].as_str().unwrap_or(""), limit);
                }
                Some("tool_use") => {
                    append(
                        output,
                        "Tool",
                        block["name"].as_str().unwrap_or("call"),
                        limit,
                    );
                    append(output, "Input", &block["input"].to_string(), limit);
                }
                Some("toolCall") => {
                    append(
                        output,
                        "Tool",
                        block["name"].as_str().unwrap_or("call"),
                        limit,
                    );
                    append(output, "Input", &block["arguments"].to_string(), limit);
                }
                Some("tool_result") => tool_result(output, &block["content"], limit),
                Some("image" | "input_image") => append(output, label, "[image]", limit),
                _ => {}
            }
        }
    }
}

pub(crate) fn tool_result(output: &mut String, value: &Value, limit: usize) {
    if let Some(text) = value.as_str() {
        append(output, "Result", text, limit);
    } else if let Some(blocks) = value.as_array() {
        for block in blocks {
            if let Some(text) = block["text"].as_str() {
                append(output, "Result", text, limit);
            }
        }
    }
}

pub(crate) fn append(output: &mut String, label: &str, text: &str, limit: usize) {
    // Plain text only: no transcript-controlled terminal escapes or C1 controls.
    let remaining = limit.saturating_sub(output.chars().count());
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
