use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::duration::{format_duration, parse_duration};
use crate::paths::Context;
use crate::status::{AgentStatus, Source};
use crate::store::ListedSession;
use crate::tmux::{self, Pane};
use crate::{attach, db, git, list, pick, resume};

const TICK: Duration = Duration::from_millis(200);
const REFRESH: Duration = Duration::from_secs(1);
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(150);
const DEFAULT_IDLE: Duration = Duration::from_secs(20 * 60);

const HINTS: &str =
    "q quit · j/k move · / filter · ga/gr view · a attach · r resume · I idle · = zoom";

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
}

#[derive(Debug)]
pub(crate) enum Effect {
    None,
    Quit,
    Attach(Pane),
    Resume(ListedSession),
}

struct App {
    view: View,
    sessions: Vec<ListedSession>,
    panes: Vec<Pane>,
    selected: usize,
    selected_pane: Option<Pane>,
    preview: Option<String>,
    preview_pending_since: Option<Instant>,
    preview_maximized: bool,
    filter: String,
    input: Input,
    idle_enabled: bool,
    idle: Duration,
    idle_input: String,
    pending_g: bool,
    message: Option<String>,
    dirty: bool,
}

impl App {
    fn new() -> Self {
        Self {
            view: View::Active,
            sessions: Vec::new(),
            panes: Vec::new(),
            selected: 0,
            selected_pane: None,
            preview: None,
            preview_pending_since: None,
            preview_maximized: false,
            filter: String::new(),
            input: Input::Normal,
            idle_enabled: false,
            idle: DEFAULT_IDLE,
            idle_input: String::new(),
            pending_g: false,
            message: None,
            dirty: false,
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
        if let Err(err) = self.load(ctx) {
            self.message = Some(format!("{err:#}"));
        }
    }

    fn load(&mut self, ctx: &Context) -> Result<()> {
        let (resumable, idle) = match self.view {
            View::Active => (false, None),
            View::Resumable => (true, self.idle_enabled.then_some(self.idle)),
        };
        let keep = self
            .selected_session()
            .map(|session| (session.provider.clone(), session.session_id.clone()));
        let mut sessions = db::query_sessions(ctx, resumable, idle)?;
        sort_sessions(&mut sessions);
        self.panes = tmux::list_panes(true).unwrap_or_default();
        self.sessions = sessions;
        self.restore_selection(keep);
        // Refresh captures immediately so the preview stays live.
        if self.selected_pane.is_some() {
            self.preview_pending_since.get_or_insert_with(Instant::now);
        }
        self.capture_preview();
        Ok(())
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
        if pane.as_ref().map(|pane| &pane.id) != self.selected_pane.as_ref().map(|pane| &pane.id) {
            self.preview_pending_since = Some(Instant::now());
        }
        self.selected_pane = pane;
    }

    fn capture_preview(&mut self) {
        if self.preview_pending_since.take().is_none() {
            return;
        }
        self.preview = self
            .selected_pane
            .as_ref()
            .and_then(|pane| tmux::capture_pane(&pane.id).ok());
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

    fn handle_key(&mut self, key: KeyEvent) -> Effect {
        if key.kind != KeyEventKind::Press {
            return Effect::None;
        }
        self.message = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Effect::Quit;
        }
        match self.input {
            Input::Filter => return self.handle_filter_key(key),
            Input::Idle => return self.handle_idle_key(key),
            Input::Normal => {}
        }
        if self.pending_g {
            self.pending_g = false;
            match key.code {
                KeyCode::Char('a') => self.set_view(View::Active),
                KeyCode::Char('r') => self.set_view(View::Resumable),
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
                self.pending_g = true;
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
        if self.selected_pane.is_none() {
            self.message = Some("no tmux preview for the selected session".into());
            return;
        }
        self.preview_maximized = !self.preview_maximized;
    }

    fn attach_selected(&mut self) -> Effect {
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
        guard.terminal.draw(|frame| render(frame, &app))?;
        if event::poll(TICK).context("poll terminal events")?
            && let Event::Key(key) = event::read().context("read terminal event")?
        {
            match app.handle_key(key) {
                Effect::None => {}
                Effect::Quit => break,
                Effect::Attach(pane) => attach_effect(&mut guard, &mut app, &pane),
                Effect::Resume(session) => match resume_session(ctx, &session) {
                    Ok(pane) => attach_effect(&mut guard, &mut app, &pane),
                    Err(err) => app.message = Some(format!("{err:#}")),
                },
            }
        }
        if app.dirty {
            app.refresh(ctx);
            app.dirty = false;
            last_refresh = Instant::now();
        }
        if app
            .preview_pending_since
            .is_some_and(|since| since.elapsed() >= PREVIEW_DEBOUNCE)
        {
            app.capture_preview();
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

fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if app.preview_maximized && app.selected_pane.is_some() {
        render_preview(frame, app, area);
        return;
    }
    let rows = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(area);
    let cols =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).split(rows[0]);
    render_list(frame, app, cols[0]);
    if app.selected_pane.is_some() {
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
    let title_width = (area.width as usize).saturating_sub(2 + 2 + 8 + 1).max(1);
    let items: Vec<ListItem> = sessions
        .iter()
        .map(|session| {
            let title = list::truncate(session.title.as_deref().unwrap_or("-"), title_width);
            ListItem::new(Line::from(vec![
                Span::styled(logo(&session.provider), Style::default().fg(Color::Magenta)),
                Span::raw(" "),
                Span::styled(
                    format!("{:<7}", session.status.as_str()),
                    status_style(session.status),
                ),
                Span::raw(" "),
                Span::raw(title),
            ]))
        })
        .collect();
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
        .block(Block::bordered().title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default();
    if count > 0 {
        state.select(Some(app.selected));
    }
    frame.render_stateful_widget(list_widget, area, &mut state);
}

fn render_details(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title(" Details ");
    let Some(session) = app.selected_session() else {
        frame.render_widget(Paragraph::new("No session selected").block(block), area);
        return;
    };
    let cwd = session.cwd.clone().unwrap_or_else(|| "-".into());
    let branch = git::branch(std::path::Path::new(&cwd)).unwrap_or_else(|| "-".into());
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
        ("title", session.title.clone().unwrap_or_else(|| "-".into())),
        ("age", age),
        ("command", command),
    ];
    if let Some(pane) = &app.selected_pane {
        rows.push((
            "tmux",
            format!("{}:{} ({})", pane.session, pane.window_name, pane.id),
        ));
    }
    let lines: Vec<Line> = rows
        .into_iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(format!("{label:<9}"), Style::default().fg(Color::DarkGray)),
                Span::raw(value),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_preview(frame: &mut Frame, app: &App, area: Rect) {
    let title = match &app.selected_pane {
        Some(pane) => format!(" {}:{} ({}) ", pane.session, pane.window_name, pane.id),
        None => " Preview ".into(),
    };
    let text = app.preview.clone().unwrap_or_default();
    let inner_height = area.height.saturating_sub(2) as usize;
    let scroll = text.lines().count().saturating_sub(inner_height) as u16;
    let preview = Paragraph::new(text)
        .block(Block::bordered().title(title))
        .scroll((scroll, 0));
    frame.render_widget(preview, area);
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
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
        Input::Normal => {
            let text = if app.pending_g {
                "g — a active sessions · r resumable sessions".to_string()
            } else {
                app.message.clone().unwrap_or_else(|| HINTS.into())
            };
            frame.render_widget(Paragraph::new(text), area);
        }
    }
}

// Newest first; ties broken deterministically so the order is stable.
fn sort_sessions(sessions: &mut [ListedSession]) {
    sessions.sort_by(|a, b| {
        b.created_ms
            .cmp(&a.created_ms)
            .then_with(|| (&a.provider, &a.session_id).cmp(&(&b.provider, &b.session_id)))
    });
}

fn logo(provider: &str) -> &'static str {
    match provider {
        "claude" => "✻",
        "codex" => "⌘",
        "grok" => "▲",
        "opencode" => "◆",
        "pi" => "π",
        _ => "●",
    }
}

fn status_style(status: AgentStatus) -> Style {
    let color = match status {
        AgentStatus::Working => Color::Green,
        AgentStatus::Waiting => Color::Yellow,
        AgentStatus::Idle => Color::Cyan,
        AgentStatus::Done => Color::DarkGray,
    };
    Style::default().fg(color)
}

#[cfg(test)]
#[path = "tui_tests.rs"]
mod tests;
