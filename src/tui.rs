use std::collections::HashMap;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ansi_to_tui::IntoText;
use anyhow::{Context as _, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::duration::{format_duration, parse_duration};
use crate::hubs::tmux::resolve_session_pane;
use crate::hubs::{self, Hub, Preview, Tmux};
use crate::paths::Context;
use crate::providers::{self, logo};
use crate::status::{AgentStatus, Source};
use crate::store::ListedSession;
use crate::tmux::{self, Pane};
use crate::{db, git, list, pick, process, resume, transcript};

const TICK: Duration = Duration::from_millis(200);
const REFRESH: Duration = Duration::from_secs(1);
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(150);
const DEFAULT_IDLE: Duration = Duration::from_secs(20 * 60);
const CACHE_CAP: usize = 64;

mod latte {
    use ratatui::style::Color;

    pub const BASE: Color = Color::Rgb(239, 241, 245);
    pub const MANTLE: Color = Color::Rgb(230, 233, 239);
    pub const SURFACE0: Color = Color::Rgb(204, 208, 218);
    pub const SURFACE2: Color = Color::Rgb(172, 176, 190);
    pub const TEXT: Color = Color::Rgb(76, 79, 105);
    pub const SUBTEXT1: Color = Color::Rgb(92, 95, 119);
    pub const MAUVE: Color = Color::Rgb(136, 57, 239);
    pub const RED: Color = Color::Rgb(210, 15, 57);
    pub const GREEN: Color = Color::Rgb(64, 160, 43);
    pub const YELLOW: Color = Color::Rgb(223, 142, 29);
    pub const TEAL: Color = Color::Rgb(23, 146, 153);
    pub const BLUE: Color = Color::Rgb(30, 102, 245);
}

const HINTS: &str = "? help · j/k move · / filter · q quit";

const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "List",
        &[
            ("j / k, ↓ / ↑", "move selection"),
            ("g a / g r", "active / resumable sessions"),
            ("/", "filter sessions (enter applies, esc clears)"),
            (
                "w t/r/a/s/d/n",
                "group by created/activity/provider/status/directory (n off)",
            ),
            (
                "s t/r/a/s/d",
                "sort by created/activity/provider/status/directory",
            ),
        ],
    ),
    (
        "Session",
        &[
            ("a", "attach or focus"),
            ("r", "resume in tmux"),
            ("d", "delete (y confirms)"),
            ("m", "edit memo"),
            ("M", "message the session"),
            ("I", "toggle idle filter (resumable)"),
        ],
    ),
    (
        "View",
        &[
            ("t", "expand tool and reasoning details"),
            ("=", "zoom preview"),
            ("␣ g g", "open lazygit"),
            ("b r / b p", "open repo / pr in browser"),
            ("?", "toggle this help"),
            ("q / ctrl-c", "quit"),
        ],
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    Created,
    Activity,
    Provider,
    Status,
    Directory,
}

impl Sort {
    fn label(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Activity => "activity",
            Self::Provider => "provider",
            Self::Status => "status",
            Self::Directory => "directory",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Active,
    Resumable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Normal,
    Filter,
    Idle,
    Memo,
    Message,
    Confirm,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    G,
    S,
    W,
    Space,
    SpaceG,
    B,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Browse {
    Repo,
    Pr,
}

#[derive(Debug)]
pub(crate) enum Effect {
    None,
    Quit,
    Focus {
        session: Box<ListedSession>,
        hub: Box<dyn Hub>,
    },
    Resume(ListedSession),
    Remove(ListedSession),
    Memo(ListedSession, String),
    Send(ListedSession, String),
    Lazygit(PathBuf),
    Browse(PathBuf, Browse),
}

/// Blocking I/O (db, tmux, git) runs on worker threads so the UI never stalls.
enum Fetch {
    Focus {
        ctx: Box<Context>,
        session: Box<ListedSession>,
        hub: Box<dyn Hub>,
    },
    Send {
        session: Box<ListedSession>,
        hub: Box<dyn Hub>,
        message: String,
    },
    Refresh(Box<RefreshRequest>),
    Preview {
        pane_id: String,
        generation: u64,
    },
    Transcript {
        source: transcript::Source,
        reader: Box<transcript::Reader>,
        details: bool,
        generation: u64,
    },
    Git {
        cwd: String,
        generation: u64,
    },
}

struct RefreshRequest {
    ctx: Context,
    resumable: bool,
    idle: Option<Duration>,
    generation: u64,
}

#[derive(Clone)]
struct CachedGit {
    branch: Option<String>,
    info: Option<git::RepoInfo>,
}

struct CachedTranscript {
    generation: u64,
    details: bool,
    content: String,
}

enum Fetched {
    Focus(Result<()>),
    Send {
        label: String,
        result: Result<()>,
    },
    Transcript {
        source: transcript::Source,
        reader: Box<transcript::Reader>,
        content: String,
        details: bool,
        generation: u64,
    },
    Refresh {
        generation: u64,
        result: Result<(Vec<ListedSession>, Vec<Pane>)>,
    },
    Preview {
        generation: u64,
        pane_id: String,
        content: Option<String>,
    },
    Git {
        generation: u64,
        cwd: String,
        branch: Option<String>,
        info: Option<git::RepoInfo>,
    },
}

fn spawn_fetch(tx: &Sender<Fetched>, fetch: Fetch) {
    let tx = tx.clone();
    thread::spawn(move || {
        let fetched = match fetch {
            Fetch::Focus { ctx, session, hub } => {
                Fetched::Focus(providers::focus(&ctx, hub.as_ref(), &session))
            }
            Fetch::Send {
                session,
                hub,
                message,
            } => {
                let label = format!("{} {}", session.provider, session.session_id);
                Fetched::Send {
                    label,
                    result: providers::send(hub.as_ref(), &session, &message),
                }
            }
            Fetch::Refresh(request) => {
                let RefreshRequest {
                    ctx,
                    resumable,
                    idle,
                    generation,
                } = *request;
                let result = (|| {
                    let sessions = db::query_sessions(&ctx, resumable, idle)?;
                    let panes = tmux::list_panes(true).unwrap_or_default();
                    Ok((sessions, panes))
                })();
                Fetched::Refresh { generation, result }
            }
            Fetch::Preview {
                pane_id,
                generation,
            } => {
                let content = tmux::capture_pane(&pane_id).ok();
                Fetched::Preview {
                    generation,
                    pane_id,
                    content,
                }
            }
            Fetch::Transcript {
                source,
                mut reader,
                details,
                generation,
            } => {
                let content = reader.preview(&source, transcript::Style::conversation(details));
                Fetched::Transcript {
                    source,
                    reader,
                    content,
                    details,
                    generation,
                }
            }
            Fetch::Git { cwd, generation } => {
                let path = Path::new(&cwd);
                let branch = git::branch(path);
                let info = git::repo_info(path);
                Fetched::Git {
                    generation,
                    cwd,
                    branch,
                    info,
                }
            }
        };
        let _ = tx.send(fetched);
    });
}

struct App {
    view: View,
    sort: Sort,
    group: Option<Sort>,
    sessions: Vec<ListedSession>,
    panes: Vec<Pane>,
    selected: usize,
    selected_pane: Option<Pane>,
    /// The selected pane shows the selected session's own output. False when
    /// the pane only hosts the session as a background tab.
    pane_shows_session: bool,
    selected_transcript: Option<transcript::Source>,
    transcript_reader: Box<transcript::Reader>,
    ctx: Context,
    preview: Option<String>,
    preview_pending_since: Option<Instant>,
    preview_in_flight: Option<u64>,
    preview_maximized: bool,
    /// Show tool calls and reasoning in conversation previews.
    show_details: bool,
    filter: String,
    input: Input,
    idle_enabled: bool,
    idle: Duration,
    idle_input: String,
    memo_input: String,
    memo_session: Option<ListedSession>,
    message_input: String,
    message_session: Option<ListedSession>,
    confirm_session: Option<ListedSession>,
    pending: Option<Pending>,
    git: Option<git::RepoInfo>,
    branch: Option<String>,
    git_cwd: Option<String>,
    git_pending_since: Option<Instant>,
    message: Option<String>,
    dirty: bool,
    needs_redraw: bool,
    fetch_tx: Sender<Fetched>,
    fetch_rx: Receiver<Fetched>,
    refresh_generation: u64,
    preview_generation: u64,
    git_generation: u64,
    previews: HashMap<String, String>,
    transcript_previews: HashMap<transcript::Source, CachedTranscript>,
    gits: HashMap<String, CachedGit>,
}

impl App {
    fn new(ctx: &Context) -> Self {
        let (fetch_tx, fetch_rx) = mpsc::channel();
        Self {
            view: View::Active,
            sort: Sort::Created,
            group: None,
            sessions: Vec::new(),
            panes: Vec::new(),
            selected: 0,
            selected_pane: None,
            // update_selection owns this flag; tests that drive select_preview
            // directly mean the pane to show the selected session.
            pane_shows_session: true,
            selected_transcript: None,
            transcript_reader: Box::default(),
            ctx: ctx.clone(),
            preview: None,
            preview_pending_since: None,
            preview_in_flight: None,
            preview_maximized: false,
            show_details: false,
            filter: String::new(),
            input: Input::Normal,
            idle_enabled: false,
            idle: DEFAULT_IDLE,
            idle_input: String::new(),
            memo_input: String::new(),
            memo_session: None,
            message_input: String::new(),
            message_session: None,
            confirm_session: None,
            pending: None,
            git: None,
            branch: None,
            git_cwd: None,
            git_pending_since: None,
            message: None,
            dirty: false,
            needs_redraw: true,
            fetch_tx,
            fetch_rx,
            refresh_generation: 0,
            preview_generation: 0,
            git_generation: 0,
            previews: HashMap::new(),
            transcript_previews: HashMap::new(),
            gits: HashMap::new(),
        }
    }

    fn filtered(&self) -> Vec<&ListedSession> {
        self.sessions
            .iter()
            .filter(|session| {
                self.filter.is_empty() || pick::matches(session, std::slice::from_ref(&self.filter))
            })
            .collect()
    }

    fn selected_session(&self) -> Option<&ListedSession> {
        self.filtered().get(self.selected).copied()
    }

    fn refresh(&mut self) {
        self.refresh_generation += 1;
        let (resumable, idle) = match self.view {
            View::Active => (false, None),
            View::Resumable => (true, self.idle_enabled.then_some(self.idle)),
        };
        spawn_fetch(
            &self.fetch_tx,
            Fetch::Refresh(Box::new(RefreshRequest {
                ctx: self.ctx.clone(),
                resumable,
                idle,
                generation: self.refresh_generation,
            })),
        );
    }

    fn drain_fetched(&mut self) {
        while let Ok(fetched) = self.fetch_rx.try_recv() {
            self.handle_fetched(fetched);
        }
    }

    fn handle_fetched(&mut self, fetched: Fetched) {
        match fetched {
            Fetched::Focus(result) => {
                self.message = Some(match result {
                    Ok(()) => "sent focus request".into(),
                    Err(err) => format!("focus: {err:#}"),
                });
                self.needs_redraw = true;
            }
            Fetched::Send { label, result } => {
                self.message = Some(match result {
                    Ok(()) => format!("sent to {label}"),
                    Err(err) => format!("send: {err:#}"),
                });
                self.needs_redraw = true;
            }
            Fetched::Transcript {
                source,
                reader,
                content,
                details,
                generation,
            } => {
                // A style change makes an older result stale, but its reader
                // still holds the stream or log position to continue from.
                if details != self.show_details {
                    if self.selected_transcript.as_ref() == Some(&source) {
                        self.transcript_reader = reader;
                    }
                    if self.preview_in_flight == Some(generation) {
                        self.preview_in_flight = None;
                    }
                    return;
                }
                // A fetch finishing after navigation can still warm the cache,
                // but must not replace a newer result for the same source.
                if self
                    .transcript_previews
                    .get(&source)
                    .is_none_or(|cached| generation > cached.generation)
                {
                    if self.transcript_previews.len() >= CACHE_CAP
                        && !self.transcript_previews.contains_key(&source)
                    {
                        self.transcript_previews.clear();
                    }
                    self.transcript_previews.insert(
                        source.clone(),
                        CachedTranscript {
                            generation,
                            details,
                            content: content.clone(),
                        },
                    );
                }
                if self.preview_in_flight != Some(generation) {
                    return;
                }
                self.preview_in_flight = None;
                if self.selected_transcript.as_ref() == Some(&source) {
                    self.transcript_reader = reader;
                    if self.preview.as_ref() != Some(&content) {
                        self.preview = Some(content);
                        self.needs_redraw = true;
                    }
                }
            }
            Fetched::Refresh { generation, result } => {
                if generation != self.refresh_generation {
                    return;
                }
                match result {
                    Ok((sessions, panes)) => self.apply_refresh(sessions, panes),
                    Err(err) => {
                        self.message = Some(format!("{err:#}"));
                        self.needs_redraw = true;
                    }
                }
            }
            Fetched::Preview {
                generation,
                pane_id,
                content,
            } => {
                if self.preview_in_flight != Some(generation) {
                    return;
                }
                self.preview_in_flight = None;
                let Some(content) = content else {
                    return;
                };
                if self.previews.get(&pane_id) != Some(&content) {
                    if self.previews.len() >= CACHE_CAP && !self.previews.contains_key(&pane_id) {
                        self.previews.clear();
                    }
                    self.previews.insert(pane_id.clone(), content.clone());
                }
                if self
                    .selected_pane
                    .as_ref()
                    .is_some_and(|pane| pane.id == pane_id)
                    && self.pane_shows_session
                    && self.preview.as_ref() != Some(&content)
                {
                    self.preview = Some(content);
                    self.needs_redraw = true;
                }
            }
            Fetched::Git {
                generation,
                cwd,
                branch,
                info,
            } => {
                if self.gits.len() >= CACHE_CAP {
                    self.gits.clear();
                }
                self.gits.insert(
                    cwd.clone(),
                    CachedGit {
                        branch: branch.clone(),
                        info: info.clone(),
                    },
                );
                if generation != self.git_generation {
                    return;
                }
                if self.git_cwd.as_deref() == Some(cwd.as_str()) {
                    self.branch = branch;
                    self.git = info;
                    self.needs_redraw = true;
                }
            }
        }
    }

    fn apply_refresh(&mut self, sessions: Vec<ListedSession>, panes: Vec<Pane>) {
        let keep = self
            .selected_session()
            .map(|session| (session.provider.clone(), session.session_id.clone()));
        self.needs_redraw = true;
        self.panes = panes;
        self.sessions = sessions;
        sort_sessions(&mut self.sessions, self.sort, self.group);
        self.restore_selection(keep);
        if self.has_preview() && self.preview_in_flight.is_none() {
            self.preview_pending_since.get_or_insert_with(Instant::now);
        }
        // Refresh git status immediately so the details stay live.
        if self.git_cwd.is_some() {
            self.git_pending_since.get_or_insert_with(Instant::now);
        }
        self.request_git();
    }

    // Follow the previously selected session across reorders; fall back to
    // clamping the index when it is gone.
    fn restore_selection(&mut self, keep: Option<(String, String)>) {
        if let Some((provider, session_id)) = keep
            && let Some(pos) = self.filtered().iter().position(|session| {
                session.provider == provider && session.session_id == session_id
            })
        {
            self.selected = pos;
        }
        self.clamp_selection();
        self.update_selection();
    }

    fn clamp_selection(&mut self) {
        let len = self.filtered().len();
        self.selected = self.selected.min(len.saturating_sub(1));
    }

    fn update_selection(&mut self) {
        let (resolved, preview) = match self.selected_session() {
            Some(session) => {
                let resolved = resolve_session_pane(&self.panes, session, std::process::id())
                    .map(|resolved| (resolved.pane.clone(), resolved.own_content));
                let hub = hubs::resolve(session, || {
                    let (pane, own_content) = resolved.clone().unzip();
                    Tmux::new(pane, own_content.unwrap_or(false))
                });
                let preview = providers::preview(&self.ctx, hub.as_ref(), session);
                (resolved, preview)
            }
            None => (None, None),
        };
        self.pane_shows_session = matches!(preview, Some(Preview::Pane(_)));
        self.select_preview(resolved.map(|(pane, _)| pane));
        let transcript = match preview {
            Some(Preview::Log(source)) => Some(source),
            _ => None,
        };
        if transcript != self.selected_transcript {
            self.selected_transcript = transcript;
            *self.transcript_reader = Default::default();
            // Invalidate any transcript or pane fetch from the old selection.
            self.preview_in_flight = None;
            self.preview_pending_since = self.has_preview().then(Instant::now);
            self.preview = self
                .selected_transcript
                .as_ref()
                .and_then(|source| {
                    self.transcript_previews
                        .get(source)
                        .filter(|cached| cached.details == self.show_details)
                        .map(|cached| cached.content.clone())
                })
                .or_else(|| {
                    self.selected_pane
                        .as_ref()
                        .filter(|_| self.pane_shows_session)
                        .and_then(|pane| self.previews.get(&pane.id).cloned())
                });
            self.needs_redraw = true;
        }
        let cwd = self
            .selected_session()
            .and_then(|session| session.cwd.clone())
            .filter(|cwd| !cwd.is_empty());
        if cwd != self.git_cwd {
            self.git_cwd = cwd;
            let cached = self.git_cwd.as_ref().and_then(|cwd| self.gits.get(cwd));
            self.branch = cached.and_then(|cached| cached.branch.clone());
            self.git = cached.and_then(|cached| cached.info.clone());
            self.git_pending_since = Some(Instant::now());
        }
    }

    fn has_preview(&self) -> bool {
        self.pane_shows_session && self.selected_pane.is_some()
            || self.selected_transcript.is_some()
    }

    fn select_preview(&mut self, pane: Option<Pane>) {
        if pane != self.selected_pane {
            self.needs_redraw = true;
        }
        // A selected transcript owns the preview content; overwriting it with
        // the (irrelevant) pane cache would flash "Loading" on every refresh
        // until the next transcript fetch returns.
        if self.selected_transcript.is_none() {
            let new_preview = pane
                .as_ref()
                .filter(|_| self.pane_shows_session)
                .and_then(|pane| self.previews.get(&pane.id).cloned());
            if new_preview != self.preview {
                self.preview = new_preview;
                self.needs_redraw = true;
            }
        }
        if pane.as_ref().map(|pane| &pane.id) != self.selected_pane.as_ref().map(|pane| &pane.id) {
            self.preview_pending_since = pane.as_ref().map(|_| Instant::now());
        }
        self.selected_pane = pane;
    }

    fn next_preview_fetch(&mut self, now: Instant) -> Option<Fetch> {
        if self.preview_in_flight.is_some()
            || self
                .preview_pending_since
                .is_none_or(|since| now.duration_since(since) < PREVIEW_DEBOUNCE)
        {
            return None;
        }
        self.preview_pending_since = None;
        if !self.has_preview() {
            return None;
        }
        self.preview_generation += 1;
        self.preview_in_flight = Some(self.preview_generation);
        if let Some(source) = &self.selected_transcript {
            return Some(Fetch::Transcript {
                source: source.clone(),
                reader: std::mem::take(&mut self.transcript_reader),
                details: self.show_details,
                generation: self.preview_generation,
            });
        }
        Some(Fetch::Preview {
            pane_id: self.selected_pane.as_ref()?.id.clone(),
            generation: self.preview_generation,
        })
    }

    fn request_preview(&mut self) {
        if let Some(fetch) = self.next_preview_fetch(Instant::now()) {
            spawn_fetch(&self.fetch_tx, fetch);
        }
    }

    fn request_git(&mut self) {
        if self.git_pending_since.take().is_none() {
            return;
        }
        let Some(cwd) = self.git_cwd.clone() else {
            self.git = None;
            self.branch = None;
            return;
        };
        self.git_generation += 1;
        spawn_fetch(
            &self.fetch_tx,
            Fetch::Git {
                cwd,
                generation: self.git_generation,
            },
        );
    }

    fn move_by(&mut self, delta: i32) {
        let len = self.filtered().len() as i32;
        if len == 0 {
            return;
        }
        self.selected = (self.selected as i32 + delta).clamp(0, len - 1) as usize;
        self.update_selection();
    }

    fn set_view(&mut self, view: View) {
        if self.view == view {
            return;
        }
        self.view = view;
        self.selected = 0;
        self.dirty = true;
    }

    fn set_sort(&mut self, sort: Sort) {
        self.sort = sort;
        self.resort();
    }

    fn set_group(&mut self, group: Option<Sort>) {
        self.group = group;
        self.resort();
    }

    fn resort(&mut self) {
        let keep = self
            .selected_session()
            .map(|session| (session.provider.clone(), session.session_id.clone()));
        sort_sessions(&mut self.sessions, self.sort, self.group);
        self.restore_selection(keep);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Effect {
        if key.kind != KeyEventKind::Press {
            return Effect::None;
        }
        self.needs_redraw = true;
        self.message = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Effect::Quit;
        }
        match self.input {
            Input::Filter => return self.handle_filter_key(key),
            Input::Idle => return self.handle_idle_key(key),
            Input::Memo => return self.handle_memo_key(key),
            Input::Message => return self.handle_message_key(key),
            Input::Confirm => return self.handle_confirm_key(key),
            Input::Help => {
                self.input = Input::Normal;
                return Effect::None;
            }
            Input::Normal => {}
        }
        if let Some(pending) = self.pending.take() {
            match (pending, key.code) {
                (Pending::G, KeyCode::Char('a')) => self.set_view(View::Active),
                (Pending::G, KeyCode::Char('r')) => self.set_view(View::Resumable),
                (Pending::S, KeyCode::Char('t')) => self.set_sort(Sort::Created),
                (Pending::S, KeyCode::Char('r')) => self.set_sort(Sort::Activity),
                (Pending::S, KeyCode::Char('a')) => self.set_sort(Sort::Provider),
                (Pending::S, KeyCode::Char('s')) => self.set_sort(Sort::Status),
                (Pending::S, KeyCode::Char('d')) => self.set_sort(Sort::Directory),
                (Pending::W, KeyCode::Char('t')) => self.set_group(Some(Sort::Created)),
                (Pending::W, KeyCode::Char('r')) => self.set_group(Some(Sort::Activity)),
                (Pending::W, KeyCode::Char('a')) => self.set_group(Some(Sort::Provider)),
                (Pending::W, KeyCode::Char('s')) => self.set_group(Some(Sort::Status)),
                (Pending::W, KeyCode::Char('d')) => self.set_group(Some(Sort::Directory)),
                (Pending::W, KeyCode::Char('n')) => self.set_group(None),
                (Pending::Space, KeyCode::Char('g')) => self.pending = Some(Pending::SpaceG),
                (Pending::SpaceG, KeyCode::Char('g')) => return self.open_lazygit(),
                (Pending::B, KeyCode::Char('r')) => return self.browse_selected(Browse::Repo),
                (Pending::B, KeyCode::Char('p')) => return self.browse_selected(Browse::Pr),
                _ => {}
            }
            return Effect::None;
        }
        match key.code {
            KeyCode::Char('q') => Effect::Quit,
            KeyCode::Esc => {
                if self.filter.is_empty() {
                    Effect::Quit
                } else {
                    self.filter.clear();
                    self.clamp_selection();
                    self.update_selection();
                    Effect::None
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_by(1);
                Effect::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_by(-1);
                Effect::None
            }
            KeyCode::Char('g') => {
                self.pending = Some(Pending::G);
                Effect::None
            }
            KeyCode::Char('s') => {
                self.pending = Some(Pending::S);
                Effect::None
            }
            KeyCode::Char('w') => {
                self.pending = Some(Pending::W);
                Effect::None
            }
            KeyCode::Char(' ') => {
                self.pending = Some(Pending::Space);
                Effect::None
            }
            KeyCode::Char('b') => {
                self.pending = Some(Pending::B);
                Effect::None
            }
            KeyCode::Char('/') => {
                self.input = Input::Filter;
                Effect::None
            }
            KeyCode::Char('I') => {
                self.toggle_idle();
                Effect::None
            }
            KeyCode::Char('t') => {
                self.toggle_details();
                Effect::None
            }
            KeyCode::Char('=') => {
                self.toggle_maximized();
                Effect::None
            }
            KeyCode::Char('a') => self.attach_selected(),
            KeyCode::Char('r') => self.resume_selected(),
            KeyCode::Char('d') => self.confirm_remove(),
            KeyCode::Char('m') => self.edit_memo(),
            KeyCode::Char('M') => self.compose_message(),
            KeyCode::Char('?') => {
                self.input = Input::Help;
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn handle_filter_key(&mut self, key: KeyEvent) -> Effect {
        match key.code {
            KeyCode::Enter => self.input = Input::Normal,
            KeyCode::Esc => {
                self.filter.clear();
                self.input = Input::Normal;
                self.selected = 0;
                self.update_selection();
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
                self.update_selection();
            }
            KeyCode::Char(ch) => {
                self.filter.push(ch);
                self.selected = 0;
                self.update_selection();
            }
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            _ => {}
        }
        Effect::None
    }

    fn handle_idle_key(&mut self, key: KeyEvent) -> Effect {
        match key.code {
            KeyCode::Enter => match parse_duration(&self.idle_input) {
                Ok(idle) => {
                    self.idle = idle;
                    self.idle_enabled = true;
                    self.input = Input::Normal;
                    self.dirty = true;
                }
                Err(err) => self.message = Some(format!("{err:#}")),
            },
            KeyCode::Esc => self.input = Input::Normal,
            KeyCode::Backspace => {
                self.idle_input.pop();
            }
            KeyCode::Char(ch) => self.idle_input.push(ch),
            _ => {}
        }
        Effect::None
    }

    fn handle_memo_key(&mut self, key: KeyEvent) -> Effect {
        match key.code {
            KeyCode::Enter => {
                self.input = Input::Normal;
                return match self.memo_session.take() {
                    Some(session) => Effect::Memo(session, std::mem::take(&mut self.memo_input)),
                    None => Effect::None,
                };
            }
            KeyCode::Esc => {
                self.input = Input::Normal;
                self.memo_session = None;
                self.memo_input.clear();
            }
            KeyCode::Backspace => {
                self.memo_input.pop();
            }
            KeyCode::Char(ch) => self.memo_input.push(ch),
            _ => {}
        }
        Effect::None
    }

    fn handle_message_key(&mut self, key: KeyEvent) -> Effect {
        match key.code {
            KeyCode::Enter => {
                if self.message_input.trim().is_empty() {
                    self.message = Some("message is empty".into());
                    return Effect::None;
                }
                self.input = Input::Normal;
                return match self.message_session.take() {
                    Some(session) => Effect::Send(session, std::mem::take(&mut self.message_input)),
                    None => Effect::None,
                };
            }
            KeyCode::Esc => {
                self.input = Input::Normal;
                self.message_session = None;
                self.message_input.clear();
            }
            KeyCode::Backspace => {
                self.message_input.pop();
            }
            KeyCode::Char(ch) => self.message_input.push(ch),
            _ => {}
        }
        Effect::None
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) -> Effect {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.input = Input::Normal;
                match self.confirm_session.take() {
                    Some(session) => Effect::Remove(session),
                    None => Effect::None,
                }
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.input = Input::Normal;
                self.confirm_session = None;
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn toggle_idle(&mut self) {
        if self.view != View::Resumable {
            self.message = Some("idle filter applies to the resumable list (gr)".into());
            return;
        }
        if self.idle_enabled {
            self.idle_enabled = false;
            self.dirty = true;
        } else {
            self.idle_input = format_duration(self.idle);
            self.input = Input::Idle;
        }
    }

    fn toggle_maximized(&mut self) {
        if !self.has_preview() {
            self.message = Some("no preview for the selected session".into());
            return;
        }
        self.preview_maximized = !self.preview_maximized;
    }

    fn toggle_details(&mut self) {
        self.show_details = !self.show_details;
        self.transcript_previews.clear();
        if self.selected_transcript.is_some() {
            let style = transcript::Style::conversation(self.show_details);
            self.preview = self.transcript_reader.render_cached(style);
            self.preview_pending_since = Some(Instant::now());
        }
        self.message = Some(if self.show_details {
            "showing tool and thinking details".into()
        } else {
            "hiding tool and thinking details".into()
        });
    }

    fn attach_selected(&mut self) -> Effect {
        let Some(session) = self.selected_session().cloned() else {
            return Effect::None;
        };
        let hub = hubs::resolve(&session, || {
            Tmux::new(self.selected_pane.clone(), self.pane_shows_session)
        });
        Effect::Focus {
            session: Box::new(session),
            hub,
        }
    }

    fn resume_selected(&mut self) -> Effect {
        if self.view != View::Resumable {
            self.message = Some("press gr to open resumable sessions".into());
            return Effect::None;
        }
        match self.selected_session().cloned() {
            Some(session) => Effect::Resume(session),
            None => {
                self.message = Some("no session selected".into());
                Effect::None
            }
        }
    }

    fn confirm_remove(&mut self) -> Effect {
        match self.selected_session().cloned() {
            Some(session) => {
                self.confirm_session = Some(session);
                self.input = Input::Confirm;
            }
            None => self.message = Some("no session selected".into()),
        }
        Effect::None
    }

    fn edit_memo(&mut self) -> Effect {
        match self.selected_session().cloned() {
            Some(session) => {
                self.memo_input = session.memo.clone().unwrap_or_default();
                self.memo_session = Some(session);
                self.input = Input::Memo;
            }
            None => self.message = Some("no session selected".into()),
        }
        Effect::None
    }

    fn compose_message(&mut self) -> Effect {
        if self.view != View::Active {
            self.message = Some("press ga to return to active sessions".into());
            return Effect::None;
        }
        match self.selected_session().cloned() {
            Some(session) if session.status == AgentStatus::Waiting => {
                self.message = Some(format!(
                    "{} {} is waiting on a permission prompt or a question; answer it with a",
                    session.provider, session.session_id
                ));
            }
            Some(session) => {
                self.message_input.clear();
                self.message_session = Some(session);
                self.input = Input::Message;
            }
            None => self.message = Some("no session selected".into()),
        }
        Effect::None
    }

    fn open_lazygit(&mut self) -> Effect {
        match self.selected_cwd() {
            Some(cwd) => Effect::Lazygit(cwd),
            None => {
                self.message = Some("session has no working directory".into());
                Effect::None
            }
        }
    }

    fn browse_selected(&mut self, target: Browse) -> Effect {
        match self.selected_cwd() {
            Some(cwd) => Effect::Browse(cwd, target),
            None => {
                self.message = Some("session has no working directory".into());
                Effect::None
            }
        }
    }

    fn selected_cwd(&self) -> Option<PathBuf> {
        self.selected_session()
            .and_then(|session| session.cwd.clone())
            .filter(|cwd| !cwd.is_empty())
            .map(PathBuf::from)
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout)).context("init terminal")?;
        Ok(Self { terminal })
    }

    fn suspend(&mut self) -> Result<()> {
        disable_raw_mode().context("disable raw mode")?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)
            .context("leave alternate screen")?;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        enable_raw_mode().context("enable raw mode")?;
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)
            .context("enter alternate screen")?;
        self.terminal.clear().context("clear terminal")?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
    }
}

pub fn run_tmux(ctx: &Context) -> Result<()> {
    if !tmux::available() {
        bail!("tmux is required for --tmux");
    }
    let name = ctx
        .berth_bin
        .file_stem()
        .and_then(|name| name.to_str())
        .context("invalid agent-berth binary path")?;
    let me = std::process::id();
    // Without a running tmux server there are no panes; the spawn below
    // creates the session.
    let panes = tmux::list_panes(true).unwrap_or_default();
    if let Some(pane) = panes
        .iter()
        .find(|pane| process::tree_contains_named(pane.pid, name, me))
    {
        return tmux::attach_pane(pane);
    }
    let cwd = std::env::current_dir().context("current directory")?;
    let cmd = vec![ctx.berth_bin.display().to_string(), "tui".into()];
    let pane_id = tmux::spawn_window_default("berth", &cwd, &cmd)?;
    let pane = tmux::find_pane(&pane_id)?.context("new TUI window not found")?;
    tmux::attach_pane(&pane)
}

pub fn run(ctx: &Context) -> Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let mut app = App::new(ctx);
    app.refresh();
    let mut last_refresh = Instant::now();
    loop {
        if last_refresh.elapsed() >= REFRESH {
            app.refresh();
            last_refresh = Instant::now();
        }
        app.drain_fetched();
        if app.needs_redraw {
            guard.terminal.draw(|frame| render(frame, &app))?;
            app.needs_redraw = false;
        }
        if event::poll(TICK).context("poll terminal events")? {
            match event::read().context("read terminal event")? {
                Event::Key(key) => match app.handle_key(key) {
                    Effect::None => {}
                    Effect::Quit => break,
                    Effect::Focus { session, hub } if hub.focus_in_foreground(&session) => {
                        attach_effect(&mut guard, &mut app, || {
                            providers::focus(ctx, hub.as_ref(), &session)
                        });
                    }
                    Effect::Focus { session, hub } => spawn_fetch(
                        &app.fetch_tx,
                        Fetch::Focus {
                            ctx: Box::new(ctx.clone()),
                            session,
                            hub,
                        },
                    ),
                    Effect::Resume(session) => match resume_session(ctx, &session) {
                        Ok(pane) => {
                            attach_effect(&mut guard, &mut app, || tmux::attach_pane(&pane))
                        }
                        Err(err) => app.message = Some(format!("{err:#}")),
                    },
                    Effect::Remove(session) => remove_effect(&mut app, ctx, &session),
                    Effect::Memo(session, memo) => memo_effect(&mut app, ctx, &session, &memo),
                    Effect::Send(session, message) => {
                        let hub = hubs::resolve(&session, Tmux::lookup);
                        spawn_fetch(
                            &app.fetch_tx,
                            Fetch::Send {
                                session: Box::new(session),
                                hub,
                                message,
                            },
                        );
                    }
                    Effect::Lazygit(cwd) => lazygit_effect(&mut guard, &mut app, &cwd),
                    Effect::Browse(cwd, target) => browse_effect(&mut app, &cwd, target),
                },
                Event::Resize(..) => app.needs_redraw = true,
                _ => {}
            }
        }
        if app.dirty {
            app.refresh();
            app.dirty = false;
            last_refresh = Instant::now();
        }
        app.request_preview();
        if app
            .git_pending_since
            .is_some_and(|since| since.elapsed() >= PREVIEW_DEBOUNCE)
        {
            app.request_git();
        }
    }
    Ok(())
}

/// Runs `attach`, which drives this terminal's tmux client, releasing the
/// terminal to it unless the client only switches panes.
fn attach_effect(guard: &mut TerminalGuard, app: &mut App, attach: impl FnOnce() -> Result<()>) {
    let result = if tmux::should_switch() {
        attach()
    } else {
        let _ = guard.suspend();
        let result = attach();
        let resumed = guard.resume();
        result.and(resumed)
    };
    if let Err(err) = result {
        app.message = Some(format!("attach: {err:#}"));
    }
    app.dirty = true;
}

fn lazygit_effect(guard: &mut TerminalGuard, app: &mut App, cwd: &Path) {
    let _ = guard.suspend();
    let result = std::process::Command::new("lazygit")
        .current_dir(cwd)
        .status()
        .context("run lazygit")
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                bail!("lazygit exited with {status}");
            }
        });
    let resumed = guard.resume();
    if let Err(err) = result.and(resumed) {
        app.message = Some(format!("{err:#}"));
    }
    app.dirty = true;
}

fn browse_effect(app: &mut App, cwd: &Path, target: Browse) {
    let args = match target {
        Browse::Repo => ["repo", "view", "-w"],
        Browse::Pr => ["pr", "view", "-w"],
    };
    match std::process::Command::new("gh")
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.trim();
            app.message = Some(if detail.is_empty() {
                format!("gh {} {} failed", args[0], args[1])
            } else {
                format!("gh: {detail}")
            });
        }
        Err(err) => app.message = Some(format!("gh: {err:#}")),
    }
}

fn resume_session(ctx: &Context, session: &ListedSession) -> Result<Pane> {
    if session.source == Source::Desktop {
        bail!("desktop sessions cannot be resumed");
    }
    let cwd = session
        .cwd
        .clone()
        .filter(|cwd| !cwd.is_empty())
        .context("session has no working directory")?;
    if !tmux::available() {
        bail!("tmux is required to resume CLI sessions");
    }
    let cwd = PathBuf::from(cwd);
    let name = tmux::ensure_session(&cwd)?;
    let window = resume::window_name(session);
    let pane_id = tmux::spawn_window(&name, &window, &cwd, &session.cmdline, &[])?;
    db::mark_resumed(ctx, &session.provider, &session.session_id)?;
    tmux::find_pane(&pane_id)?.context("resumed pane not found")
}

fn remove_effect(app: &mut App, ctx: &Context, session: &ListedSession) {
    match db::mark_removed(ctx, &session.provider, &session.session_id) {
        Ok(()) => {
            app.message = Some(format!(
                "removed {} {}",
                session.provider, session.session_id
            ));
            app.dirty = true;
        }
        Err(err) => app.message = Some(format!("remove: {err:#}")),
    }
}

fn memo_effect(app: &mut App, ctx: &Context, session: &ListedSession, memo: &str) {
    match db::set_memo(ctx, &session.provider, &session.session_id, memo) {
        Ok(()) => {
            app.message = Some(if memo.trim().is_empty() {
                format!(
                    "cleared memo for {} {}",
                    session.provider, session.session_id
                )
            } else {
                format!("memo set for {} {}", session.provider, session.session_id)
            });
            app.dirty = true;
        }
        Err(err) => app.message = Some(format!("memo: {err:#}")),
    }
}

fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().fg(latte::TEXT).bg(latte::BASE)),
        area,
    );
    if app.preview_maximized && app.has_preview() {
        render_preview(frame, app, area);
    } else {
        let rows = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(area);
        let cols = Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(rows[0]);
        render_list(frame, app, cols[0]);
        if app.has_preview() {
            let right = Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)])
                .split(cols[1]);
            render_details(frame, app, right[0]);
            render_preview(frame, app, right[1]);
        } else {
            render_details(frame, app, cols[1]);
        }
        render_footer(frame, app, rows[1]);
    }
    if app.input == Input::Help {
        render_help(frame, area);
    }
}

fn help_lines() -> Vec<Line<'static>> {
    let key_width = HELP
        .iter()
        .flat_map(|(_, rows)| rows.iter().map(|(keys, _)| keys.chars().count()))
        .max()
        .unwrap_or(0);
    let mut lines = Vec::new();
    for (index, (section, rows)) in HELP.iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        lines.push(Line::from(Span::styled(
            *section,
            Style::default()
                .fg(latte::MAUVE)
                .add_modifier(Modifier::BOLD),
        )));
        for (keys, description) in *rows {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {keys:<key_width$}  "),
                    Style::default().fg(latte::YELLOW),
                ),
                Span::styled(*description, Style::default().fg(latte::TEXT)),
            ]));
        }
    }
    lines
}

fn render_help(frame: &mut Frame, area: Rect) {
    let lines = help_lines();
    let width = lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .saturating_add(2) as u16;
    let height = (lines.len() as u16).saturating_add(2);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" Keyboard shortcuts ")
                .style(Style::default().bg(latte::MANTLE)),
        ),
        popup,
    );
}

fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let sessions = app.filtered();
    let title_width = (area.width as usize)
        .saturating_sub(2 + 1 + 2 + 2 + 1)
        .max(1);
    let now = crate::store::now_ms();
    let mut items = Vec::new();
    let mut previous_group = None;
    let mut selected_row = None;
    for (index, session) in sessions.iter().enumerate() {
        if let Some(group_by) = app.group {
            let group = session_group(session, group_by, now);
            if previous_group.as_ref() != Some(&group) {
                let label = if group_by == Sort::Directory {
                    crate::paths::worktree_label(&group)
                } else {
                    group.clone()
                };
                let label = list::truncate(&label, area.width.saturating_sub(4) as usize);
                items.push(
                    ListItem::new(format!(" {label} ")).style(
                        Style::default()
                            .fg(latte::BLUE)
                            .bg(latte::MANTLE)
                            .add_modifier(Modifier::BOLD),
                    ),
                );
                previous_group = Some(group);
            }
        }
        let selected = index == app.selected;
        if selected {
            selected_row = Some(items.len());
        }
        let title = list::truncate(session.display_title().unwrap_or("-"), title_width);
        // The highlight only paints the row background; bolding the logo glyph
        // would make terminals pick a bold fallback font for it, which draws
        // the glyph at a different offset than the regular one.
        let emphasis = if selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        items.push(ListItem::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(logo(&session.provider), Style::default().fg(latte::MAUVE)),
            Span::raw(" "),
            Span::styled(
                session.status.as_str()[..1].to_ascii_uppercase(),
                status_style(session.status).patch(emphasis),
            ),
            Span::raw(" "),
            Span::styled(title, emphasis),
            Span::raw(" "),
        ])));
    }
    let count = sessions.len();
    let mut title = match app.view {
        View::Active => format!(" Active ({count}) "),
        View::Resumable => format!(" Resumable ({count}) "),
    };
    if app.view == View::Resumable && app.idle_enabled {
        title = format!(
            "{}· idle ≤ {} ",
            title.trim_end(),
            format_duration(app.idle)
        );
    }
    if !app.filter.is_empty() {
        title = format!("{}· /{} ", title.trim_end(), app.filter);
    }
    let mut footer = format!(" sort: {} ", app.sort.label());
    if let Some(group) = app.group {
        footer = format!("{}· group: {} ", footer, group.label());
    }
    let list_widget = List::new(items)
        .block(Block::bordered().title(title).title_bottom(footer))
        .highlight_style(Style::default().bg(latte::SURFACE0));
    let mut state = ListState::default();
    state.select(selected_row);
    frame.render_stateful_widget(list_widget, area, &mut state);
}

fn session_group(session: &ListedSession, group: Sort, now: u64) -> String {
    match group {
        Sort::Created | Sort::Activity => {
            let timestamp = if group == Sort::Created {
                session.created_ms
            } else {
                session.last_report_ms
            };
            match now.saturating_sub(timestamp) {
                0..=3_600_000 => "1h",
                3_600_001..=86_400_000 => "1d",
                86_400_001..=604_800_000 => "7d",
                _ => ">7d",
            }
            .into()
        }
        Sort::Provider => session.provider.clone(),
        Sort::Status => session.status.as_str().into(),
        Sort::Directory => session.cwd.clone().unwrap_or_else(|| "-".into()),
    }
}

fn render_details(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title(" Details ");
    let Some(session) = app.selected_session() else {
        frame.render_widget(Paragraph::new("No session selected").block(block), area);
        return;
    };
    let cwd = session.cwd.clone().unwrap_or_else(|| "-".into());
    let branch = app
        .git
        .as_ref()
        .map(|info| info.branch.clone())
        .or_else(|| app.branch.clone())
        .unwrap_or_else(|| "-".into());
    let ago = list::ago_label(session.last_report_ms);
    let command = if session.cmdline.is_empty() {
        "-".into()
    } else {
        session.cmdline.join(" ")
    };
    let mut rows = vec![
        ("id", session.id()),
        ("provider", session.provider.clone()),
        ("session", session.session_id.clone()),
        ("status", session.status.as_str().to_string()),
        ("source", session.source.as_str().to_string()),
        (
            "pid",
            session
                .pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "-".into()),
        ),
        ("cwd", cwd),
        ("branch", branch),
    ];
    if let Some(repo) = app.git.as_ref().and_then(|info| info.github.as_ref()) {
        rows.push(("repo", repo.clone()));
    }
    rows.extend([
        ("title", session.title.clone().unwrap_or_else(|| "-".into())),
        ("ago", ago),
        ("command", command),
    ]);
    if let Some(memo) = &session.memo {
        rows.push(("memo", memo.clone()));
    }
    if let Some(pane) = &app.selected_pane {
        rows.push((
            "tmux",
            format!("{}:{} ({})", pane.session, pane.window_name, pane.id),
        ));
    }
    let lines: Vec<Line> = rows
        .into_iter()
        .map(|(label, value)| {
            let mut spans = vec![Span::styled(
                format!("{label:<9}"),
                Style::default().fg(latte::SUBTEXT1),
            )];
            if label == "branch" {
                spans.extend(branch_spans(
                    &value,
                    app.git.as_ref().map(|info| &info.status),
                ));
                return Line::from(spans);
            }
            let style = match label {
                "provider" => Style::default().fg(latte::MAUVE),
                "status" => status_style(session.status),
                "cwd" | "repo" => Style::default().fg(latte::BLUE),
                "source" | "tmux" => Style::default().fg(latte::TEAL),
                "title" | "memo" => Style::default().add_modifier(Modifier::BOLD),
                _ => Style::default(),
            };
            spans.push(Span::styled(value, style));
            Line::from(spans)
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn branch_spans(branch: &str, status: Option<&git::RepoStatus>) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        branch.to_string(),
        Style::default().fg(latte::GREEN),
    )];
    let Some(status) = status else {
        return spans;
    };
    let bracket_style = Style::default()
        .fg(latte::SURFACE2)
        .add_modifier(Modifier::BOLD);
    let (tracking_symbol, tracking_color) = match status.tracking {
        Some((0, 0)) => ("≡", latte::GREEN),
        Some((_, 0)) => ("↑", latte::RED),
        Some((0, _)) => ("↓", latte::RED),
        Some(_) => ("↕", latte::RED),
        None => ("", latte::RED),
    };
    for (present, symbol, color, bold) in [
        (status.conflicted, "!", latte::RED, false),
        (status.stashed, "$", latte::RED, true),
        (status.deleted, "✘", latte::RED, true),
        (status.renamed, "»", latte::YELLOW, true),
        (status.modified, "!", latte::YELLOW, true),
        (status.staged, "+", latte::GREEN, true),
        (status.untracked, "?", latte::RED, true),
        (
            status.tracking.is_some(),
            tracking_symbol,
            tracking_color,
            true,
        ),
    ] {
        if !present {
            continue;
        }
        if spans.len() == 1 {
            spans.push(Span::styled(" [", bracket_style));
        }
        let mut style = Style::default().fg(color);
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(symbol, style));
    }
    if spans.len() > 1 {
        spans.push(Span::styled("]", bracket_style));
    }
    spans
}

fn render_preview(frame: &mut Frame, app: &App, area: Rect) {
    // A selected transcript always provides the content, even when the
    // session's pane exists but only hosts it as a background tab.
    let title = match (&app.selected_transcript, &app.selected_pane) {
        (Some(_), _) => " Conversation · live transcript ".into(),
        (None, Some(pane)) => format!(" {}:{} ({}) ", pane.session, pane.window_name, pane.id),
        (None, None) => " Preview ".into(),
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let transcript = app.selected_transcript.is_some();
    let content = app.preview.as_deref().unwrap_or(if transcript {
        "Loading conversation…"
    } else {
        ""
    });
    let text = if transcript {
        Text::raw(content)
    } else {
        content
            .into_text()
            .unwrap_or_else(|_| Text::raw("Unable to render pane preview"))
    };
    let mut preview = Paragraph::new(text);
    if transcript {
        preview = preview.wrap(Wrap { trim: false });
    } else {
        preview = preview.style(Style::default().fg(Color::Reset).bg(Color::Reset));
    }
    let scroll = preview
        .line_count(inner.width)
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    let preview = preview.scroll((scroll, 0));
    frame.render_widget(preview, inner);
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().fg(latte::TEXT).bg(latte::MANTLE)),
        area,
    );
    match app.input {
        Input::Filter => {
            frame.render_widget(Paragraph::new(format!("/{}", app.filter)), area);
            frame.set_cursor_position((area.x + 1 + app.filter.len() as u16, area.y));
        }
        Input::Idle => {
            let prompt = format!("idle within: {}", app.idle_input);
            frame.render_widget(Paragraph::new(prompt), area);
            frame.set_cursor_position((area.x + 13 + app.idle_input.len() as u16, area.y));
        }
        Input::Memo => {
            let prompt = format!("memo: {}", app.memo_input);
            frame.render_widget(Paragraph::new(prompt), area);
            frame.set_cursor_position((area.x + 6 + app.memo_input.len() as u16, area.y));
        }
        Input::Message => {
            let target = app
                .message_session
                .as_ref()
                .map(|session| session.id())
                .unwrap_or_default();
            let prefix = format!("message to {target}: ");
            frame.render_widget(
                Paragraph::new(format!("{prefix}{}", app.message_input)),
                area,
            );
            frame.set_cursor_position((
                area.x + prefix.len() as u16 + app.message_input.len() as u16,
                area.y,
            ));
        }
        Input::Confirm => {
            let label = match &app.confirm_session {
                Some(session) => format!("{} {}", session.provider, session.session_id),
                None => "session".into(),
            };
            frame.render_widget(
                Paragraph::new(format!("delete {label}? (y/N)"))
                    .style(Style::default().fg(latte::RED)),
                area,
            );
        }
        Input::Normal | Input::Help => {
            let text = match app.pending {
                Some(Pending::G) => "g — a active sessions · r resumable sessions".to_string(),
                Some(Pending::S) => {
                    "s — t creation · r recent activity · a agent provider · s status · d directory"
                        .to_string()
                }
                Some(Pending::W) => {
                    "w — t creation · r recent activity · a agent provider · s status · d directory · n off"
                        .to_string()
                }
                Some(Pending::Space) | Some(Pending::SpaceG) => "␣g — g open lazygit".to_string(),
                Some(Pending::B) => "b — r repo in browser · p pr in browser".to_string(),
                None => app.message.clone().unwrap_or_else(|| HINTS.into()),
            };
            frame.render_widget(Paragraph::new(text), area);
        }
    }
}

fn sort_sessions(sessions: &mut [ListedSession], sort: Sort, group: Option<Sort>) {
    let home = (sort == Sort::Directory)
        .then(|| crate::paths::home_dir().ok())
        .flatten();
    let group_home = (group == Some(Sort::Directory))
        .then(|| crate::paths::home_dir().ok())
        .flatten();
    sessions.sort_by(|a, b| {
        let group_order = match group {
            Some(criterion) => compare_sessions(a, b, criterion, group_home.as_deref()),
            None => std::cmp::Ordering::Equal,
        };
        group_order
            .then_with(|| compare_sessions(a, b, sort, home.as_deref()))
            .then_with(|| b.created_ms.cmp(&a.created_ms))
            .then_with(|| (&a.provider, &a.session_id).cmp(&(&b.provider, &b.session_id)))
    });
}

fn compare_sessions(
    a: &ListedSession,
    b: &ListedSession,
    criterion: Sort,
    home: Option<&Path>,
) -> std::cmp::Ordering {
    match criterion {
        Sort::Created => b.created_ms.cmp(&a.created_ms),
        Sort::Activity => b.last_report_ms.cmp(&a.last_report_ms),
        Sort::Provider => a.provider.cmp(&b.provider),
        Sort::Status => a.status.rank().cmp(&b.status.rank()),
        Sort::Directory => (
            a.cwd.is_none(),
            a.cwd
                .as_deref()
                .map(|dir| crate::paths::worktree_label_with_home(dir, home)),
            &a.cwd,
        )
            .cmp(&(
                b.cwd.is_none(),
                b.cwd
                    .as_deref()
                    .map(|dir| crate::paths::worktree_label_with_home(dir, home)),
                &b.cwd,
            )),
    }
}

fn status_style(status: AgentStatus) -> Style {
    let color = match status {
        AgentStatus::Running => latte::GREEN,
        AgentStatus::Waiting => latte::YELLOW,
        AgentStatus::Idle => latte::TEAL,
        AgentStatus::Done => latte::SUBTEXT1,
    };
    Style::default().fg(color)
}

#[cfg(test)]
#[path = "tui_tests.rs"]
mod tests;
