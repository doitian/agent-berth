use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

const APP: &str = "agent-berth";
const PIPE: &str = "agent-berth";

#[derive(Debug, Clone)]
pub struct Context {
    pub home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_runtime_dir: PathBuf,
    pub state_dir: PathBuf,
    pub claude_config_dir: PathBuf,
    pub codex_home: PathBuf,
    pub grok_home: PathBuf,
    pub pi_dir: PathBuf,
    pub appdata: Option<PathBuf>,
    pub berth_bin: PathBuf,
    pub socket_override: Option<String>,
}

impl Context {
    pub fn from_env() -> Result<Self> {
        let home = home_dir()?;
        let xdg_config_home = env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let xdg_runtime_dir = env_path("XDG_RUNTIME_DIR").unwrap_or_else(|| {
            #[cfg(windows)]
            {
                env_path("TEMP").unwrap_or_else(|| home.join("AppData/Local/Temp"))
            }
            #[cfg(not(windows))]
            {
                std::env::temp_dir()
            }
        });
        let state_dir = env_path("XDG_STATE_HOME")
            .map(|p| p.join(APP))
            .or_else(|| env_path("LOCALAPPDATA").map(|p| p.join(APP)))
            .unwrap_or_else(|| home.join(".local/state").join(APP));
        Ok(Self {
            claude_config_dir: env_path("CLAUDE_CONFIG_DIR")
                .unwrap_or_else(|| home.join(".claude")),
            codex_home: env_path("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            grok_home: env_path("GROK_HOME").unwrap_or_else(|| home.join(".grok")),
            pi_dir: env_path("PI_CODING_AGENT_DIR")
                .unwrap_or_else(|| home.join(".pi").join("agent")),
            appdata: env_path("APPDATA"),
            berth_bin: env::current_exe().unwrap_or_else(|_| PathBuf::from(APP)),
            socket_override: env::var("AGENT_BERTH_SOCK").ok(),
            home,
            xdg_config_home,
            xdg_runtime_dir,
            state_dir,
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.state_dir.join("state.redb")
    }

    pub fn json_legacy_path(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }

    pub fn pid_path(&self) -> PathBuf {
        self.state_dir.join("server.pid")
    }

    pub fn socket_path(&self) -> PathBuf {
        if let Some(value) = &self.socket_override {
            return PathBuf::from(value);
        }
        self.xdg_runtime_dir.join("agent-berth.sock")
    }

    pub fn pipe_name(&self) -> String {
        self.socket_override
            .clone()
            .unwrap_or_else(|| PIPE.to_string())
    }

    pub fn endpoint_display(&self) -> String {
        #[cfg(windows)]
        {
            format!(r"\\.\pipe\{}", self.pipe_name())
        }
        #[cfg(unix)]
        {
            self.socket_path().display().to_string()
        }
    }

    pub fn opencode_plugin_dir(&self) -> PathBuf {
        self.xdg_config_home.join("opencode").join("plugins")
    }

    pub fn systemd_unit_path(&self) -> PathBuf {
        self.xdg_config_home
            .join("systemd")
            .join("user")
            .join("agent-berth.service")
    }

    pub fn quote_bin(&self) -> String {
        quote_path(&self.berth_bin)
    }

    pub fn notify_command(&self, provider: &str) -> String {
        format!("{} notify --provider {provider}", self.quote_bin())
    }
}

pub fn home_dir() -> Result<PathBuf> {
    env_path("HOME")
        .or_else(|| env_path("USERPROFILE"))
        .context("HOME or USERPROFILE is not set")
}

/// Short label for a working directory, normalizing git worktree layouts to
/// `name` or `name (worktree)` so a repo and its worktrees sort together.
pub fn worktree_label(path: &str) -> String {
    worktree_label_with_home(path, home_dir().ok().as_deref())
}

pub(crate) fn worktree_label_with_home(path: &str, home: Option<&Path>) -> String {
    let normalized = path.replace('\\', "/");
    let components: Vec<&str> = normalized.split('/').filter(|c| !c.is_empty()).collect();
    let Some(&dir) = components.last() else {
        return if normalized.contains('/') { "/" } else { "-" }.into();
    };
    let Some(wt) = components[..components.len() - 1]
        .iter()
        .rposition(|c| is_worktrees_dir(c))
    else {
        return dir.into();
    };
    let name = components[wt];
    if name.len() > ".worktrees".len() && name.ends_with(".worktrees") {
        let base = &name[..name.len() - ".worktrees".len()];
        return format!("{base} ({dir})");
    }
    if is_central_worktrees(&components, wt, home) {
        // $HOME/.something/worktrees/<group>/<dir>
        return if components.len() - 2 > wt {
            format!("{dir} ({})", components[components.len() - 2])
        } else {
            dir.into()
        };
    }
    // Worktree folder nested inside the main checkout, e.g. .claude/worktrees.
    match components[..wt].iter().rev().find(|c| !c.starts_with('.')) {
        Some(main) => format!("{main} ({dir})"),
        None => dir.into(),
    }
}

fn is_worktrees_dir(component: &str) -> bool {
    component == "worktrees" || component.ends_with(".worktrees")
}

fn is_central_worktrees(components: &[&str], wt: usize, home: Option<&Path>) -> bool {
    let Some(home) = home else { return false };
    let home = home.display().to_string().replace('\\', "/");
    let home_components: Vec<&str> = home.split('/').filter(|c| !c.is_empty()).collect();
    wt > home_components.len()
        && components.len() > home_components.len()
        && components[..home_components.len()] == home_components[..]
        && components[home_components.len()].starts_with('.')
}

pub fn env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn quote_path(path: &Path) -> String {
    let text = path.display().to_string();
    // Hook commands run under bash (claude), cmd (grok), or codex's own
    // command splitting. An unquoted forward-slash path works in all of them:
    // bash mangles unquoted backslashes, and codex cannot parse quoted paths.
    // Paths containing spaces are not supported on Windows.
    if cfg!(windows) {
        return text.replace('\\', "/");
    }
    if text.chars().any(|ch| ch.is_whitespace() || ch == '"') {
        format!("\"{}\"", text.replace('"', "\\\""))
    } else {
        text
    }
}

pub fn is_current_dir(cwd: &str) -> bool {
    if cwd.is_empty() {
        return false;
    }
    let Ok(current) = env::current_dir() else {
        return false;
    };
    normalize(&current) == normalize(Path::new(cwd))
}

fn normalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn on_path(name: &str) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat", ".com"]
    } else {
        &[""]
    };
    env::split_paths(&path).any(|dir| {
        exts.iter().any(|ext| {
            let candidate = dir.join(format!("{name}{ext}"));
            candidate.is_file()
        })
    })
}

/// The first file on PATH that `name` can spawn. npm installs `.cmd` shims on
/// Windows, which `Command::new(name)` cannot resolve because CreateProcess
/// only appends `.exe`.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    find_executable_in(name, &env::var_os("PATH")?)
}

fn find_executable_in(name: &str, path: &OsStr) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat", ".com"]
    } else {
        &[""]
    };
    env::split_paths(path).find_map(|dir| {
        exts.iter().find_map(|ext| {
            let candidate = dir.join(format!("{name}{ext}"));
            candidate.is_file().then_some(candidate)
        })
    })
}

#[cfg(test)]
impl Context {
    pub fn for_test(root: &Path, bin: &Path) -> Self {
        Self {
            home: root.to_path_buf(),
            xdg_config_home: root.join("config"),
            xdg_runtime_dir: root.join("run"),
            state_dir: root.join("state"),
            claude_config_dir: root.join("claude"),
            codex_home: root.join("codex"),
            grok_home: root.join("grok"),
            pi_dir: root.join("pi"),
            appdata: Some(root.join("appdata")),
            berth_bin: bin.to_path_buf(),
            socket_override: Some(
                #[cfg(windows)]
                format!("agent-berth-test-{}", std::process::id()),
                #[cfg(unix)]
                root.join("run")
                    .join("agent-berth.sock")
                    .display()
                    .to_string(),
            ),
        }
    }
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
