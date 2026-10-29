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
use serde::Serialize;
use serde_json::Value;

use crate::providers::TranscriptLog;

const READ_LIMIT: u64 = 1024 * 1024;
const RECORD_LIMIT: usize = 256 * 1024;
const ENTRY_LIMIT: usize = 48;
const TEXT_LIMIT: usize = 1024;

/// The provider log a conversation is read from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Source {
    pub provider: String,
    pub session_id: String,
    pub path: Option<String>,
    pub root: Option<PathBuf>,
}

/// One piece of a transcript record, in the order the agent produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Entry {
    /// A message asking the agent for work: the user's, or an orchestrator's.
    Prompt {
        text: String,
    },
    /// Text the agent's harness adds on the user's side, such as skill bodies,
    /// environment details, or command output.
    Context {
        text: String,
    },
    Text {
        text: String,
    },
    /// The agent's private reasoning, shown only when details are asked for.
    Thinking {
        text: String,
    },
    ToolCall {
        name: String,
        input: String,
    },
    ToolResult {
        text: String,
    },
}

impl Entry {
    fn is_output(&self) -> bool {
        matches!(
            self,
            Self::Text { .. } | Self::ToolCall { .. } | Self::ToolResult { .. }
        )
    }
}

/// One parsed log row or exported message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Record {
    pub entries: Vec<Entry>,
    /// The record asks the agent for new work, so it opens a turn unless the
    /// current turn has no output yet.
    pub starts_turn: bool,
    /// The agent finished the turn this record belongs to.
    pub ends_turn: bool,
}

/// The work the agent did for one request: its prompts, then its output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Turn {
    pub records: Vec<Record>,
    pub complete: bool,
}

impl Turn {
    fn has_output(&self) -> bool {
        self.entries().any(Entry::is_output)
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.records.iter().flat_map(|record| &record.entries)
    }

    pub fn prompts(&self) -> impl Iterator<Item = &str> {
        self.entries().filter_map(|entry| match entry {
            Entry::Prompt { text } => Some(text.as_str()),
            _ => None,
        })
    }

    /// The agent's latest reply: the last run of its text in this turn, even
    /// when tool calls followed it.
    pub fn reply(&self) -> Option<String> {
        let entries: Vec<&Entry> = self.entries().collect();
        let end = entries
            .iter()
            .rposition(|entry| matches!(entry, Entry::Text { .. }))?;
        let start = entries[..end]
            .iter()
            .rposition(|entry| !matches!(entry, Entry::Text { .. }))
            .map_or(0, |index| index + 1);
        let texts: Vec<&str> = entries[start..=end]
            .iter()
            .filter_map(|entry| match entry {
                Entry::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .filter(|text| !text.is_empty())
            .collect();
        Some(texts.join("\n\n")).filter(|text| !text.is_empty())
    }

    /// Tool calls at the end of the turn that have no result yet, such as a
    /// command waiting for permission.
    pub fn pending(&self) -> Vec<&Entry> {
        let entries: Vec<&Entry> = self.entries().collect();
        let start = entries
            .iter()
            .rposition(|entry| {
                !matches!(
                    entry,
                    Entry::ToolCall { .. } | Entry::Context { .. } | Entry::Thinking { .. }
                )
            })
            .map_or(0, |index| index + 1);
        entries[start..]
            .iter()
            .copied()
            .filter(|entry| matches!(entry, Entry::ToolCall { .. }))
            .collect()
    }
}

/// The newest `count` turns that have output, oldest first. The newest turn is
/// always included, so a prompt the agent has not answered yet shows up.
pub(crate) fn latest(turns: &[Turn], count: usize) -> Vec<(usize, &Turn)> {
    let mut picked: Vec<(usize, &Turn)> = Vec::new();
    for (index, turn) in turns.iter().enumerate().rev() {
        if picked.len() >= count {
            break;
        }
        if picked.is_empty() || turn.has_output() {
            picked.push((index, turn));
        }
    }
    picked.reverse();
    picked
}

/// How much of each tool call and result to show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Tools {
    /// Every call's input and result in full
    #[default]
    Full,
    /// One line per call and per result
    Brief,
    /// No tool calls or results, except calls still waiting for a result
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Style {
    pub tools: Tools,
    /// Show text the harness injected on the user's side.
    pub context: bool,
    /// Show the agent's reasoning in full.
    pub details: bool,
    /// Leave a one-line clue for hidden tools and reasoning.
    pub clue: bool,
}

impl Style {
    #[cfg(test)]
    pub const FULL: Self = Self {
        tools: Tools::Full,
        context: true,
        details: false,
        clue: false,
    };

    /// A conversation preview: a clue by default, full details when asked.
    pub fn conversation(details: bool) -> Self {
        Self {
            tools: if details { Tools::Full } else { Tools::None },
            context: true,
            details,
            clue: !details,
        }
    }
}

const BRIEF_LIMIT: usize = 120;

pub(crate) fn shown_tools(tools: Tools, entry: &Entry, pending: &[&Entry]) -> Tools {
    if tools == Tools::None && pending.iter().any(|call| std::ptr::eq(*call, entry)) {
        Tools::Brief
    } else {
        tools
    }
}

/// The input of a tool call on one line: its command, path, or pattern when
/// the input names one, otherwise the whole input.
pub(crate) fn brief_input(input: &str) -> String {
    const KEYS: &[&str] = &[
        "command",
        "cmd",
        "file_path",
        "path",
        "pattern",
        "query",
        "url",
        "description",
        "prompt",
    ];
    let value: Option<Value> = serde_json::from_str(input).ok();
    let named = value.as_ref().and_then(|value| {
        KEYS.iter().find_map(|key| match &value[*key] {
            Value::String(text) => Some(text.clone()),
            Value::Array(parts) => Some(
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
    });
    one_line(&named.unwrap_or_else(|| input.to_string()))
}

/// A tool result as its first line, noting how many lines follow.
pub(crate) fn brief_result(text: &str) -> String {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next().map(one_line).unwrap_or_default();
    match lines.count() {
        0 => first,
        1 => format!("{first} (+1 line)"),
        more => format!("{first} (+{more} lines)"),
    }
}

fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = flat.chars();
    let mut line: String = chars.by_ref().take(BRIEF_LIMIT).collect();
    if chars.next().is_some() {
        line.push('…');
    }
    line
}

/// A session's whole conversation, structured when the agent keeps a log it
/// can parse, otherwise plain text from wherever it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Conversation {
    Turns(Vec<Turn>),
    Text { text: String, origin: String },
}

/// Groups records into turns. A record that starts a turn joins the current
/// one while it has neither output nor an end yet, so a turn marker and its
/// prompt, or a local command that got no reply and the next prompt, stay
/// together.
pub(crate) fn turns(records: impl IntoIterator<Item = Record>) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for record in records {
        if turns.is_empty()
            || record.starts_turn
                && turns
                    .last()
                    .is_some_and(|turn| turn.complete || turn.has_output())
        {
            turns.push(Turn::default());
        }
        let turn = turns.last_mut().expect("a turn exists");
        turn.complete |= record.ends_turn;
        turn.records.push(record);
    }
    turns
}

/// Renders entries with the labels `You`, `Assistant`, `Tool`, `Input`, and
/// `Result`, cutting the text at `limit` characters in total.
#[cfg(test)]
pub(crate) fn render(entries: &[Entry], limit: usize) -> String {
    render_styled(entries.iter(), limit, Style::FULL, &[])
}

/// Like [`render`], showing tools and context as `style` asks. Calls in
/// `pending` are shown on one line even when tools are hidden, since they are
/// what a waiting agent is blocked on.
pub(crate) fn render_styled<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    limit: usize,
    style: Style,
    pending: &[&Entry],
) -> String {
    let mut output = String::new();
    for entry in entries {
        let tools = shown_tools(style.tools, entry, pending);
        match entry {
            Entry::Prompt { text } => append(&mut output, "You", text, limit),
            Entry::Context { text } if style.context => append(&mut output, "You", text, limit),
            Entry::Context { .. } => {}
            Entry::Text { text } => append(&mut output, "Assistant", text, limit),
            Entry::Thinking { text } if style.details => {
                append(&mut output, "Thinking", text, limit)
            }
            Entry::Thinking { .. } if style.clue => append(&mut output, "Thinking", "…", limit),
            Entry::Thinking { .. } => {}
            Entry::ToolCall { name, input } => match tools {
                Tools::Full => {
                    append(&mut output, "Tool", name, limit);
                    append(&mut output, "Input", input, limit);
                }
                Tools::Brief => {
                    let call = format!("{name} {}", brief_input(input));
                    append(&mut output, "Tool", call.trim_end(), limit);
                }
                Tools::None if style.clue => append(&mut output, "Tool", name, limit),
                Tools::None => {}
            },
            Entry::ToolResult { text } => match tools {
                Tools::Full => append(&mut output, "Result", text, limit),
                Tools::Brief => append(&mut output, "Result", &brief_result(text), limit),
                Tools::None => {}
            },
        }
    }
    output
}

fn log(provider: &str) -> Option<&'static dyn TranscriptLog> {
    crate::providers::lookup(provider)?.log()
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
    entries: VecDeque<(u64, Vec<Entry>)>,
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
    pub fn preview(&mut self, source: &Source, style: Style) -> String {
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
                self.render(style)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.resolved = None;
                "Transcript unavailable: the file is missing. Retrying…".into()
            }
            Err(error) => format!("Unable to read transcript ({}). Retrying…", error.kind()),
        }
    }

    /// The current preview content without reading the source again, for when
    /// only the style changes.
    pub fn render_cached(&self, style: Style) -> Option<String> {
        self.resolved.as_ref().map(|_| self.render(style))
    }

    #[cfg(test)]
    fn preview_log(&mut self, source: &Source) -> String {
        self.preview(source, Style::FULL)
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
        // A compressed log is appended a whole frame at a time, so it advances
        // past the complete frames and leaves one still being written for the
        // next read.
        let (bytes, consumed) = if log.compressed() {
            log.decode(&bytes, usize::MAX)
        } else {
            let len = bytes.len();
            (bytes, len)
        };
        self.offset += consumed as u64;
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
        // Turn markers render to nothing and must not use up the entry budget.
        let Some(entries) = parse_entries(log, &row) else {
            return;
        };
        let key = record_key(log, &row);
        if let Some((_, existing)) = self.entries.iter_mut().find(|(id, _)| *id == key) {
            *existing = entries;
        } else {
            self.entries.push_back((key, entries));
            if self.entries.len() > ENTRY_LIMIT {
                self.entries.pop_front();
                self.omitted = true;
            }
        }
    }

    fn render(&self, style: Style) -> String {
        let mut text = if self.omitted {
            "[Showing recent content; older, oversized, or malformed records omitted]\n\n".into()
        } else {
            String::new()
        };
        let all: Vec<&Entry> = self
            .entries
            .iter()
            .flat_map(|(_, entries)| entries)
            .collect();
        let pending = trailing_calls(&all);
        for (_, entries) in &self.entries {
            let rendered = render_styled(entries.iter(), TEXT_LIMIT, style, &pending);
            if !rendered.is_empty() {
                text.push_str(&rendered);
                text.push_str("\n\n");
            }
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

/// Read a whole local transcript into turns, oldest first, for one-shot CLI
/// output. Unlike `Reader`, it reads the entire file, keeps every record, and
/// does not truncate message text.
pub(crate) fn full(source: &Source) -> Result<Conversation> {
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
    let mut file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
    let mut order: Vec<u64> = Vec::new();
    let mut records: HashMap<u64, Record> = HashMap::new();
    let mut add = |line: &[u8]| {
        let Ok(row) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        let Some(record) = log.parse(&row) else {
            return;
        };
        let key = record_key(log, &row);
        if records.insert(key, record).is_none() {
            order.push(key);
        }
    };
    if log.compressed() {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .with_context(|| format!("read {}", path.display()))?;
        let (text, _) = log.decode(&bytes, usize::MAX);
        text.split(|byte| *byte == b'\n').for_each(&mut add);
    } else {
        for line in BufReader::new(file).split(b'\n') {
            add(&line.with_context(|| format!("read {}", path.display()))?);
        }
    }
    Ok(Conversation::Turns(turns(
        order.iter().filter_map(|key| records.remove(key)),
    )))
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
            } else if kind.is_file()
                && log.is_session_log(&entry.path(), id)
                && belongs_to(log, &entry.path(), id)
            {
                return Some(entry.path());
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
    // Only the first record identifies a compressed log, so stop decoding once
    // it is complete instead of expanding the whole conversation.
    let bytes = if log.compressed() {
        log.decode(&bytes, RECORD_LIMIT).0
    } else {
        bytes
    };
    bytes
        .split(|c| *c == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .any(|row| log.is_session_record(&row, session_id))
}

fn trailing_calls<'a>(entries: &[&'a Entry]) -> Vec<&'a Entry> {
    let start = entries
        .iter()
        .rposition(|entry| {
            !matches!(
                entry,
                Entry::ToolCall { .. } | Entry::Context { .. } | Entry::Thinking { .. }
            )
        })
        .map_or(0, |index| index + 1);
    entries[start..]
        .iter()
        .copied()
        .filter(|entry| matches!(entry, Entry::ToolCall { .. }))
        .collect()
}

fn parse_entries(log: &dyn TranscriptLog, row: &Value) -> Option<Vec<Entry>> {
    let record = log.parse(row)?;
    let entries: Vec<Entry> = record
        .entries
        .into_iter()
        .filter(|entry| match entry {
            Entry::Prompt { text }
            | Entry::Context { text }
            | Entry::Text { text }
            | Entry::Thinking { text }
            | Entry::ToolResult { text } => !text.is_empty(),
            Entry::ToolCall { name, .. } => !name.is_empty(),
        })
        .collect();
    (!entries.is_empty()).then_some(entries)
}

#[cfg(test)]
fn parse(log: &dyn TranscriptLog, row: &Value) -> Option<String> {
    let record = log.parse(row)?;
    Some(render(&record.entries, TEXT_LIMIT)).filter(|output| !output.is_empty())
}

/// Entries for a message's content: a plain string, or blocks of text, tool
/// calls, tool results, and images. `said` wraps the speaker's text, such as
/// `Entry::Prompt` for a user or `Entry::Text` for the agent.
pub(crate) fn content(value: &Value, said: fn(String) -> Entry) -> Vec<Entry> {
    let mut entries = Vec::new();
    if let Some(text) = value.as_str() {
        entries.push(said(text.to_string()));
    } else if let Some(blocks) = value.as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("text" | "input_text" | "output_text") => {
                    entries.push(said(block["text"].as_str().unwrap_or("").to_string()));
                }
                Some("tool_use") => entries.push(Entry::ToolCall {
                    name: block["name"].as_str().unwrap_or("call").to_string(),
                    input: block["input"].to_string(),
                }),
                Some("toolCall" | "tool-call") => entries.push(Entry::ToolCall {
                    name: block["name"].as_str().unwrap_or("call").to_string(),
                    input: tool_input(&block["arguments"]),
                }),
                Some("tool_result") => entries.extend(tool_result(&block["content"])),
                Some("thinking") => {
                    if let Some(thinking) = thinking(block) {
                        entries.push(thinking);
                    }
                }
                Some("reasoning") => {
                    if let Some(thinking) = thinking(block) {
                        entries.push(thinking);
                    }
                }
                Some("image" | "input_image") => entries.push(said("[image]".into())),
                _ => {}
            }
        }
    }
    entries
}

/// A reasoning block's text, from the field its harness uses or its content
/// blocks.
pub(crate) fn thinking(value: &Value) -> Option<Entry> {
    let direct = value["thinking"]
        .as_str()
        .or_else(|| value["text"].as_str())
        .or_else(|| value["reasoning"].as_str());
    let text = direct.map(str::to_string).unwrap_or_else(|| {
        ["content", "summary"]
            .iter()
            .filter_map(|key| value[key].as_array())
            .flat_map(|parts| parts.iter())
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    });
    (!text.is_empty()).then_some(Entry::Thinking { text })
}

/// A tool call's arguments: the text itself when the log keeps them as a JSON
/// string, otherwise their compact JSON.
fn tool_input(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

pub(crate) fn tool_result(value: &Value) -> Vec<Entry> {
    let result = |text: &str| Entry::ToolResult {
        text: text.to_string(),
    };
    if let Some(text) = value.as_str() {
        vec![result(text)]
    } else if let Some(blocks) = value.as_array() {
        blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .map(result)
            .collect()
    } else {
        Vec::new()
    }
}

/// The first text of a message's content, which tells a harness-written row
/// from the user's own words.
pub(crate) fn first_text(value: &Value) -> &str {
    value
        .as_str()
        .or_else(|| {
            value
                .as_array()?
                .iter()
                .find_map(|block| block["text"].as_str())
        })
        .unwrap_or("")
}

pub(crate) fn prompt(text: String) -> Entry {
    Entry::Prompt { text }
}

pub(crate) fn context(text: String) -> Entry {
    Entry::Context { text }
}

pub(crate) fn said(text: String) -> Entry {
    Entry::Text { text }
}

fn append(output: &mut String, label: &str, text: &str, limit: usize) {
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
pub(crate) mod tests_support {
    use super::*;

    /// A conversation as plain `logs` prints it.
    pub(crate) fn rendered(conversation: &Conversation) -> String {
        match conversation {
            Conversation::Turns(turns) => turns
                .iter()
                .flat_map(|turn| &turn.records)
                .map(|record| render(&record.entries, usize::MAX))
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
            Conversation::Text { text, .. } => text.clone(),
        }
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
