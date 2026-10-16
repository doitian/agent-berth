use std::collections::VecDeque;

use serde_json::Value;

const ENTRY_LIMIT: usize = 48;
const TEXT_LIMIT: usize = 8192;
const TOTAL_LIMIT: usize = 64 * 1024;

#[derive(Default)]
pub(super) struct Timeline {
    entries: VecDeque<Entry>,
    pub epoch: Option<String>,
    last_seq: Option<u64>,
}

struct Entry {
    label: &'static str,
    key: Option<String>,
    turn: Option<String>,
    text: String,
}

impl Timeline {
    pub fn history(&mut self, payload: &Value) {
        self.entries.clear();
        self.last_seq = None;
        self.epoch = payload["epoch"].as_str().map(str::to_string);
        if let Some(entries) = payload["entries"].as_array() {
            for entry in entries.iter().rev().take(ENTRY_LIMIT).rev() {
                self.item(
                    &entry["item"],
                    entry["seqEnd"].as_u64(),
                    entry["turnId"].as_str(),
                );
            }
        }
    }

    pub fn event(&mut self, payload: &Value) {
        let event = &payload["event"];
        if event["type"] == "timeline" {
            self.item(
                &event["item"],
                payload["seq"].as_u64(),
                event["turnId"].as_str(),
            );
        } else if let Some(fallback) = match event["type"].as_str() {
            Some("turn_failed") => Some("Turn failed"),
            Some("turn_canceled") => Some("Turn canceled"),
            _ => None,
        } {
            let message = event["error"].as_str().unwrap_or(fallback);
            self.item(
                &serde_json::json!({"type":"error", "message":message}),
                None,
                None,
            );
        }
    }

    fn item(&mut self, item: &Value, seq: Option<u64>, turn: Option<&str>) {
        // Live events overlap the history fetched after subscription acknowledgement.
        if seq.is_some_and(|seq| self.last_seq.is_some_and(|last| seq <= last)) {
            return;
        }
        if seq.is_some() {
            self.last_seq = seq;
        }
        let (label, text, key) = match item["type"].as_str() {
            Some("user_message") => (
                "User",
                item["text"].as_str().unwrap_or("").to_string(),
                None,
            ),
            Some("assistant_message") => (
                "Assistant",
                item["text"].as_str().unwrap_or("").to_string(),
                item["messageId"].as_str().map(str::to_string),
            ),
            Some("tool_call") => (
                "Tool",
                format!(
                    "{} · {}",
                    item["name"].as_str().unwrap_or("tool"),
                    item["status"].as_str().unwrap_or("running")
                ),
                item["callId"].as_str().map(str::to_string),
            ),
            Some("error") => (
                "Error",
                item["message"].as_str().unwrap_or("").to_string(),
                None,
            ),
            Some("notification") => (
                "Notice",
                item["message"].as_str().unwrap_or("").to_string(),
                None,
            ),
            _ => return,
        };
        let text = clean(&text);
        if text.is_empty() {
            return;
        }
        if label == "Tool"
            && key.is_some()
            && let Some(existing) = self
                .entries
                .iter_mut()
                .find(|entry| entry.label == label && entry.key == key)
        {
            existing.text = text;
        } else if label == "Assistant"
            && let Some(existing) = self.entries.back_mut().filter(|entry| {
                entry.label == label && entry.key == key && entry.turn.as_deref() == turn
            })
        {
            existing.text = clean(&format!("{}{text}", existing.text));
        } else {
            self.entries.push_back(Entry {
                label,
                key,
                turn: turn.map(str::to_string),
                text,
            });
        }
        while self.entries.len() > ENTRY_LIMIT
            || self
                .entries
                .iter()
                .map(|entry| entry.text.len())
                .sum::<usize>()
                > TOTAL_LIMIT
        {
            self.entries.pop_front();
        }
    }

    pub fn render(&self) -> String {
        if self.entries.is_empty() {
            return "No conversation messages yet.".into();
        }
        self.entries
            .iter()
            .map(|entry| format!("{}: {}\n\n", entry.label, entry.text))
            .collect()
    }
}

fn clean(text: &str) -> String {
    let mut text: String = text
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect();
    if text.len() > TEXT_LIMIT {
        let mut start = text.len() - TEXT_LIMIT;
        while !text.is_char_boundary(start) {
            start += 1
        }
        text.drain(..start);
    }
    text
}
