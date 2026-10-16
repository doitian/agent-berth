use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::db;
use crate::hubs::{self, Tmux};
use crate::paths::Context as AppContext;
use crate::pick;
use crate::providers;
use crate::status::AgentStatus;
use crate::store::{ListedSession, now_ms};
use crate::transcript::{
    self, Conversation, Entry, Style, Tools, Turn, brief_input, brief_result, latest, render_styled,
};

/// Lines a selection prints from a transcript without turns, such as a pane
/// capture, when `--tail` does not say how many.
const FALLBACK_LINES: usize = 40;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Select {
    #[default]
    All,
    /// The agent's latest reply only.
    Final,
    /// The newest turns that have output.
    Turns(usize),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub select: Select,
    pub tools: Tools,
    pub tail: Option<usize>,
    pub json: bool,
}

/// What `logs` prints: the transcript on stdout, and notes about what it
/// could not show on stderr, so scripts read stdout alone.
#[derive(Debug, Default, PartialEq, Eq)]
struct Output {
    stdout: String,
    notes: Vec<String>,
}

pub fn run(ctx: &AppContext, selector: &str, options: Options) -> Result<()> {
    let sessions = db::query_stored(ctx)?;
    let Some(session) = pick::single(&sessions, selector)? else {
        return Ok(());
    };
    let conversation = transcript_for(ctx, &session)?;
    let output = output(&conversation, &session, options, now_ms());
    for note in &output.notes {
        eprintln!("note: {note}");
    }
    if !output.stdout.is_empty() {
        println!("{}", output.stdout);
    }
    Ok(())
}

/// Best available transcript: the provider's own source when it has one,
/// otherwise the session's hub, such as its tmux pane.
fn transcript_for(ctx: &AppContext, session: &ListedSession) -> Result<Conversation> {
    let hub = hubs::resolve(session, Tmux::lookup);
    providers::transcript(ctx, hub.as_ref(), session).map_err(|error| {
        anyhow!(
            "no transcript for {} {}: {error:#}",
            session.provider,
            session.session_id
        )
    })
}

fn output(
    conversation: &Conversation,
    session: &ListedSession,
    options: Options,
    now: u64,
) -> Output {
    let mut notes = Vec::new();
    let name = format!("{} {}", session.provider, session.id());
    if options.select == Select::Final && session.is_active(now) {
        match session.status {
            AgentStatus::Running => notes.push(format!(
                "{name} is still working; this may not be its final reply"
            )),
            AgentStatus::Waiting => notes.push(format!(
                "{name} is waiting on a permission prompt or a question"
            )),
            AgentStatus::Done | AgentStatus::Idle => {}
        }
    }
    let stdout = match conversation {
        Conversation::Turns(turns) => {
            let answered = turns
                .last()
                .is_some_and(|turn| turn.reply().is_some() || !turn.pending().is_empty());
            if options.select == Select::Final && !answered {
                notes.push(format!("{name} has not replied to its latest prompt yet"));
            }
            if options.json {
                serde_json::to_string_pretty(&turns_json(turns, session, options))
                    .unwrap_or_default()
            } else {
                tail(render_turns(turns, options), options.tail)
            }
        }
        Conversation::Text { text, origin } => {
            let text = match options.select {
                Select::All => tail(text.clone(), options.tail),
                Select::Final | Select::Turns(_) => {
                    let lines = options.tail.unwrap_or(FALLBACK_LINES);
                    notes.push(format!(
                        "{origin} has no structured transcript; showing its last {lines} lines"
                    ));
                    tail_lines(text, lines)
                }
            };
            if options.json {
                let mut value = session_json(session, false);
                value["origin"] = origin.as_str().into();
                value["text"] = text.into();
                serde_json::to_string_pretty(&value).unwrap_or_default()
            } else {
                text
            }
        }
    };
    Output { stdout, notes }
}

fn tail(text: String, lines: Option<usize>) -> String {
    match lines {
        Some(lines) => tail_lines(&text, lines),
        None => text,
    }
}

fn selected(turns: &[Turn], select: Select) -> Vec<(usize, &Turn)> {
    match select {
        Select::All => turns.iter().enumerate().collect(),
        Select::Final => latest(turns, 1),
        Select::Turns(count) => latest(turns, count),
    }
}

fn render_turns(turns: &[Turn], options: Options) -> String {
    if options.select == Select::Final {
        let Some(turn) = turns.last() else {
            return String::new();
        };
        let mut parts: Vec<String> = turn.reply().into_iter().collect();
        parts.extend(turn.pending().into_iter().filter_map(|call| {
            match call {
                Entry::ToolCall { name, input } => Some(
                    format!("Pending: {name} {}", brief_input(input))
                        .trim_end()
                        .into(),
                ),
                _ => None,
            }
        }));
        return parts.join("\n\n");
    }
    let style = Style {
        tools: options.tools,
        // A whole transcript stays as it was; picked turns show only what
        // the user and the agent said.
        context: options.select == Select::All,
    };
    let last = turns.len().saturating_sub(1);
    selected(turns, options.select)
        .into_iter()
        .flat_map(|(index, turn)| {
            let pending = if index == last {
                turn.pending()
            } else {
                Vec::new()
            };
            turn.records
                .iter()
                .map(move |record| render_styled(&record.entries, usize::MAX, style, &pending))
                .collect::<Vec<_>>()
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn session_json(session: &ListedSession, structured: bool) -> Value {
    json!({
        "id": session.id(),
        "provider": session.provider,
        "session_id": session.session_id,
        "status": session.status.as_str(),
        "structured": structured,
    })
}

fn turns_json(turns: &[Turn], session: &ListedSession, options: Options) -> Value {
    let last = turns.len().saturating_sub(1);
    let items: Vec<Value> = selected(turns, options.select)
        .into_iter()
        .map(|(index, turn)| {
            let pending = if index == last {
                turn.pending()
            } else {
                Vec::new()
            };
            let prompts: Vec<&str> = turn.prompts().collect();
            let mut item = json!({
                "index": index,
                "complete": turn.complete,
                "prompt": (!prompts.is_empty()).then(|| prompts.join("\n\n")),
                "final": turn.reply(),
                "pending": pending.iter().map(|call| entry_json(call, Tools::Full)).collect::<Vec<_>>(),
            });
            if options.select != Select::Final {
                let context = options.select == Select::All;
                item["entries"] = turn
                    .entries()
                    .filter(|entry| context || !matches!(entry, Entry::Context { .. }))
                    .filter_map(|entry| {
                        let tools = transcript::shown_tools(options.tools, entry, &pending);
                        let tool = matches!(entry, Entry::ToolCall { .. } | Entry::ToolResult { .. });
                        (!tool || tools != Tools::None).then(|| entry_json(entry, tools))
                    })
                    .collect();
            }
            item
        })
        .collect();
    let mut value = session_json(session, true);
    value["turns"] = items.into();
    value
}

fn entry_json(entry: &Entry, tools: Tools) -> Value {
    match (entry, tools) {
        (Entry::ToolCall { name, input }, Tools::Brief) => {
            json!({"kind": "tool_call", "name": name, "input": brief_input(input)})
        }
        (Entry::ToolResult { text }, Tools::Brief) => {
            json!({"kind": "tool_result", "text": brief_result(text)})
        }
        _ => serde_json::to_value(entry).unwrap_or_default(),
    }
}

fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.trim_end_matches('\n').lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

#[cfg(test)]
#[path = "logs_tests.rs"]
mod tests;
