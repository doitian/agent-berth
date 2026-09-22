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
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::duration::{format_duration, parse_duration};
use crate::paths::Context;
use crate::status::{AgentStatus, Source};
use crate::store::ListedSession;
use crate::tmux::{self, Pane};
use crate::{attach, db, git, list, pick, process, resume, transcript};

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

const HINTS: &str = "q quit · j/k move · / filter · ga/gr view · s sort · w groups · a attach/focus · r resume · d delete · I idle · = zoom · ␣gg lazygit · br/bp browse";

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
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    G,
    S,
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
    Attach(Pane),
    FocusDesktop {
        provider: String,
        session_id: String,
    },
    Resume(ListedSession),
    Remove(ListedSession),
    Lazygit(PathBuf),
    Browse(PathBuf, Browse),
}

/// Blocking I/O (db, tmux, git) runs on worker threads so the UI never stalls.
enum Fetch {
    FocusDesktop {
        ctx: Box<Context>,
        provider: String,
        session_id: String,
    },
    Refresh(Box<RefreshRequest>),
    Preview {
        pane_id: String,
        generation: u64,
    },
    Transcript {
        source: transcript::Source,
        reader: Box<transcript::Reader>,
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
    content: String,
}

enum Fetched {
    FocusDesktop(Result<()>),
    Transcript {
        source: transcript::Source,
        reader: Box<transcript::Reader>,
        content: String,
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
            Fetch::FocusDesktop {
                ctx,
                provider,
                session_id,
            } => Fetched::FocusDesktop(crate::providers::focus_desktop(
                &ctx,
                &provider,
                &session_id,
            )),
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
                generation,
            } => {
                let content = reader.preview(&source);
                Fetched::Transcript {
                    source,
                    reader,
                    content,
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
    group_headers: bool,
    sessions: Vec<ListedSession>,
    panes: Vec<Pane>,
    selected: usize,
    selected_pane: Option<Pane>,
    selected_transcript: Option<transcript::Source>,
    transcript_reader: Box<transcript::Reader>,
    transcript_roots: HashMap<String, PathBuf>,
    preview: Option<String>,
    preview_pending_since: Option<Instant>,
    preview_in_flight: Option<u64>,
    preview_maximized: bool,
    filter: String,
    input: Input,
    idle_enabled: bool,
    idle: Duration,
    idle_input: String,
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
    fn new() -> Self {
        let (fetch_tx, fetch_rx) = mpsc::channel();
        Self {
            view: View::Active,
            sort: Sort::Created,
            group_headers: false,
            sessions: Vec::new(),
            panes: Vec::new(),
            selected: 0,
            selected_pane: None,
            selected_transcript: None,
            transcript_reader: Box::default(),
            transcript_roots: HashMap::new(),
            preview: None,
            preview_pending_since: None,
            preview_in_flight: None,
            preview_maximized: false,
            filter: String::new(),
            input: Input::Normal,
            idle_enabled: false,
            idle: DEFAULT_IDLE,
            idle_input: String::new(),
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

    fn refresh(&mut self, ctx: &Context) {
        self.transcript_roots
            .insert("claude".into(), ctx.claude_config_dir.join("projects"));
        self.transcript_roots
            .insert("codex".into(), ctx.codex_home.join("sessions"));
        self.refresh_generation += 1;
        let (resumable, idle) = match self.view {
            View::Active => (false, None),
            View::Resumable => (true, self.idle_enabled.then_some(self.idle)),
        };
        spawn_fetch(
            &self.fetch_tx,
            Fetch::Refresh(Box::new(RefreshRequest {
                ctx: ctx.clone(),
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
            Fetched::FocusDesktop(result) => {
                self.message = Some(match result {
                    Ok(()) => "sent focus request to desktop app".into(),
                    Err(err) => format!("focus: {err:#}"),
                });
                self.needs_redraw = true;
            }
            Fetched::Transcript {
                source,
                reader,
                content,
                generation,
            } => {
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
        sort_sessions(&mut self.sessions, self.sort);
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
        let pane = self
            .selected_session()
            .and_then(|session| attach::pane_for_session(&self.panes, session))
            .cloned();
        self.select_preview(pane);
        let transcript = self
            .selected_session()
            .filter(|session| {
                session.source == Source::Desktop
                    && matches!(session.provider.as_str(), "claude" | "codex")
            })
            .map(|session| transcript::Source {
                provider: session.provider.clone(),
                session_id: session.session_id.clone(),
                path: session.transcript_path.clone(),
                root: self.transcript_roots.get(&session.provider).cloned(),
            });
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
                        .map(|cached| cached.content.clone())
                })
                .or_else(|| {
                    self.selected_pane
                        .as_ref()
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
        self.selected_pane.is_some() || self.selected_transcript.is_some()
    }

    fn select_preview(&mut self, pane: Option<Pane>) {
        if pane != self.selected_pane {
            self.needs_redraw = true;
        }
        if pane.as_ref().map(|pane| &pane.id) != self.selected_pane.as_ref().map(|pane| &pane.id) {
            self.preview_pending_since = pane.as_ref().map(|_| Instant::now());
            self.preview = pane
                .as_ref()
                .and_then(|pane| self.previews.get(&pane.id).cloned());
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
        let keep = self
            .selected_session()
            .map(|session| (session.provider.clone(), session.session_id.clone()));
        self.sort = sort;
        sort_sessions(&mut self.sessions, sort);
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
            Input::Confirm => return self.handle_confirm_key(key),
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
                self.group_headers = !self.group_headers;
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
            KeyCode::Char('=') => {
                self.toggle_maximized();
                Effect::None
            }
            KeyCode::Char('a') => self.attach_selected(),
            KeyCode::Char('r') => self.resume_selected(),
            KeyCode::Char('d') => self.confirm_remove(),
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

    fn attach_selected(&mut self) -> Effect {
        if let Some(session) = self.selected_session()
            && matches!(session.provider.as_str(), "claude" | "codex")
            && session.source == Source::Desktop
        {
            return Effect::FocusDesktop {
                provider: session.provider.clone(),
                session_id: session.session_id.clone(),
            };
        }
        match self.selected_pane.clone() {
            Some(pane) => Effect::Attach(pane),
            None => {
                self.message = Some("selected session is not in a tmux pane".into());
                Effect::None
            }
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
    let mut app = App::new();
    app.refresh(ctx);
    let mut last_refresh = Instant::now();
    loop {
        if last_refresh.elapsed() >= REFRESH {
            app.refresh(ctx);
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
                    Effect::Attach(pane) => attach_effect(&mut guard, &mut app, &pane),
                    Effect::FocusDesktop {
                        provider,
                        session_id,
                    } => spawn_fetch(
                        &app.fetch_tx,
                        Fetch::FocusDesktop {
                            ctx: Box::new(ctx.clone()),
                            provider,
                            session_id,
                        },
                    ),
                    Effect::Resume(session) => match resume_session(ctx, &session) {
                        Ok(pane) => attach_effect(&mut guard, &mut app, &pane),
                        Err(err) => app.message = Some(format!("{err:#}")),
                    },
                    Effect::Remove(session) => remove_effect(&mut app, ctx, &session),
                    Effect::Lazygit(cwd) => lazygit_effect(&mut guard, &mut app, &cwd),
                    Effect::Browse(cwd, target) => browse_effect(&mut app, &cwd, target),
                },
                Event::Resize(..) => app.needs_redraw = true,
                _ => {}
            }
        }
        if app.dirty {
            app.refresh(ctx);
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

fn attach_effect(guard: &mut TerminalGuard, app: &mut App, pane: &Pane) {
    let result = if std::env::var_os("TMUX").is_some() {
        tmux::attach_pane(pane)
    } else {
        let _ = guard.suspend();
        let result = tmux::attach_pane(pane);
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
    let pane_id = tmux::spawn_window(&name, &window, &cwd, &session.cmdline)?;
    db::mark_removed(ctx, &session.provider, &session.session_id)?;
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

fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().fg(latte::TEXT).bg(latte::BASE)),
        area,
    );
    if app.preview_maximized && app.has_preview() {
        render_preview(frame, app, area);
        return;
    }
    let rows = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(area);
    let cols =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).split(rows[0]);
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
        if app.group_headers {
            let group = session_group(session, app.sort, now);
            if previous_group.as_ref() != Some(&group) {
                let label = if app.sort == Sort::Directory {
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
        if index == app.selected {
            selected_row = Some(items.len());
        }
        let title = list::truncate(session.title.as_deref().unwrap_or("-"), title_width);
        items.push(ListItem::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(logo(&session.provider), Style::default().fg(latte::MAUVE)),
            Span::raw(" "),
            Span::styled(
                session.status.as_str()[..1].to_ascii_uppercase(),
                status_style(session.status),
            ),
            Span::raw(" "),
            Span::raw(title),
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
    let list_widget = List::new(items)
        .block(
            Block::bordered()
                .title(title)
                .title_bottom(format!(" sort: {} ", app.sort.label())),
        )
        .highlight_style(
            Style::default()
                .bg(latte::SURFACE0)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select(selected_row);
    frame.render_stateful_widget(list_widget, area, &mut state);
}

fn session_group(session: &ListedSession, sort: Sort, now: u64) -> String {
    match sort {
        Sort::Created | Sort::Activity => {
            let timestamp = if sort == Sort::Created {
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
    let age = list::age_label(session.last_report_ms);
    let command = if session.cmdline.is_empty() {
        "-".into()
    } else {
        session.cmdline.join(" ")
    };
    let mut rows = vec![
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
        ("age", age),
        ("command", command),
    ]);
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
                "title" => Style::default().add_modifier(Modifier::BOLD),
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
    let title = match &app.selected_pane {
        Some(pane) => format!(" {}:{} ({}) ", pane.session, pane.window_name, pane.id),
        None if app.selected_transcript.is_some() => " Conversation · live transcript ".into(),
        None => " Preview ".into(),
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
        Input::Normal => {
            let text = match app.pending {
                Some(Pending::G) => "g — a active sessions · r resumable sessions".to_string(),
                Some(Pending::S) => {
                    "s — t creation · r recent activity · a agent provider · s status · d directory"
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

fn sort_sessions(sessions: &mut [ListedSession], sort: Sort) {
    let home = (sort == Sort::Directory)
        .then(|| crate::paths::home_dir().ok())
        .flatten();
    sessions.sort_by(|a, b| {
        let order = match sort {
            Sort::Created => b.created_ms.cmp(&a.created_ms),
            Sort::Activity => b.last_report_ms.cmp(&a.last_report_ms),
            Sort::Provider => a.provider.cmp(&b.provider),
            Sort::Status => a.status.rank().cmp(&b.status.rank()),
            Sort::Directory => (
                a.cwd.is_none(),
                a.cwd
                    .as_deref()
                    .map(|dir| crate::paths::worktree_label_with_home(dir, home.as_deref())),
                &a.cwd,
            )
                .cmp(&(
                    b.cwd.is_none(),
                    b.cwd
                        .as_deref()
                        .map(|dir| crate::paths::worktree_label_with_home(dir, home.as_deref())),
                    &b.cwd,
                )),
        };
        order
            .then_with(|| b.created_ms.cmp(&a.created_ms))
            .then_with(|| (&a.provider, &a.session_id).cmp(&(&b.provider, &b.session_id)))
    });
}

fn logo(provider: &str) -> &'static str {
    match provider {
        "claude" => "✻",
        "codex" => "⌘",
        "grok" => "𝕏",
        "opencode" => "⧈",
        "pi" => "π",
        _ => "●",
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
