use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{Duration, Instant};

use interprocess::local_socket::Stream;
use interprocess::local_socket::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;

pub mod mock_llm;

pub const BERTH: &str = env!("CARGO_BIN_EXE_agent-berth");
const TIMEOUT: Duration = Duration::from_secs(20);

fn executable(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn fixture() -> Arc<TempDir> {
    static FIXTURE: Mutex<Weak<TempDir>> = Mutex::new(Weak::new());
    let mut cached = FIXTURE.lock().unwrap();
    if let Some(fixture) = cached.upgrade() {
        return fixture;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new("rustc");
    command
        .arg("--edition=2024")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/fixture.rs"))
        .arg("-o")
        .arg(dir.path().join(executable("fixture")));
    success(&mut command);
    let fixture = Arc::new(dir);
    *cached = Arc::downgrade(&fixture);
    fixture
}

pub struct Sandbox {
    pub root: TempDir,
    pub env: BTreeMap<OsString, OsString>,
    pub endpoint: String,
    pub namespace: String,
    pub server: Option<Child>,
    _fixture: Arc<TempDir>,
}

impl Sandbox {
    pub fn new() -> Self {
        let root = tempfile::Builder::new().prefix("ab-it-").tempdir().unwrap();
        let namespace = root
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let endpoint = if cfg!(windows) {
            namespace.clone()
        } else {
            root.path().join("berth.sock").display().to_string()
        };
        let mut env = BTreeMap::new();
        for key in ["SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
            if let Some(value) = std::env::var_os(key) {
                env.insert(key.into(), value);
            }
        }
        for (key, dir) in [
            ("HOME", "home"),
            ("USERPROFILE", "home"),
            ("APPDATA", "appdata"),
            ("LOCALAPPDATA", "localappdata"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_RUNTIME_DIR", "run"),
            ("CLAUDE_CONFIG_DIR", "claude"),
            ("CLAUDE_DESKTOP_SESSIONS", "claude-desktop"),
            ("CODEX_HOME", "codex"),
            ("GROK_HOME", "grok"),
            ("PI_CODING_AGENT_DIR", "pi"),
            ("PSMUX_DATA_DIR", "psmux"),
            ("TEMP", "tmp"),
            ("TMP", "tmp"),
            ("TMPDIR", "tmp"),
            ("FIXTURE_LOG_DIR", "tmux-log"),
        ] {
            let path = root.path().join(dir);
            fs::create_dir_all(&path).unwrap();
            env.insert(key.into(), path.into_os_string());
        }
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let fixture = fixture();
        for name in [
            "claude",
            "codex",
            "fzf",
            "grok",
            "opencode",
            "pi",
            "tmux",
            "test-agent",
            "hook-recorder",
        ] {
            fs::copy(
                fixture.path().join(executable("fixture")),
                bin.join(executable(name)),
            )
            .unwrap();
        }
        env.insert("PATH".into(), bin.into_os_string());
        env.insert("AGENT_BERTH_SOCK".into(), endpoint.clone().into());
        env.insert("AGENT_BERTH_TMUX_SOCKET".into(), namespace.clone().into());
        let config = root.path().join("tmux.conf");
        fs::write(&config, "").unwrap();
        env.insert("AGENT_BERTH_TMUX_CONFIG".into(), config.into_os_string());
        env.insert(
            "FIXTURE_AGENT_REPORT".into(),
            root.path().join("agent-report.txt").into_os_string(),
        );
        fs::create_dir(root.path().join("project")).unwrap();
        fs::write(root.path().join("project/.tmux-up.conf"), "").unwrap();
        Self {
            root,
            env,
            endpoint,
            namespace,
            server: None,
            _fixture: fixture,
        }
    }

    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .envs(&self.env)
            .current_dir(self.project());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        command
    }

    pub fn berth(&self) -> Command {
        self.command(BERTH)
    }

    pub fn project(&self) -> PathBuf {
        self.root.path().join("project")
    }

    pub fn start(&mut self) {
        assert!(self.server.is_none());
        let log = fs::File::create(self.root.path().join("server.log")).unwrap();
        self.server = Some(
            self.berth()
                .arg("server")
                .stdout(Stdio::null())
                .stderr(log)
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if self.try_request(json!({"op": "ping"})).is_ok() {
                break;
            }
            let status = self.server.as_mut().unwrap().try_wait().unwrap();
            assert!(
                status.is_none() && Instant::now() < deadline,
                "server failed to start: {status:?}\n{}",
                self.server_log()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.server.take() {
            let _ = child.kill();
            child.wait().unwrap();
        }
    }

    pub fn server_log(&self) -> String {
        fs::read_to_string(self.root.path().join("server.log")).unwrap_or_default()
    }

    fn try_request(&self, request: Value) -> Result<Value, String> {
        let endpoint = self.endpoint.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Value> {
                #[cfg(windows)]
                let name =
                    endpoint.to_ns_name::<interprocess::local_socket::GenericNamespaced>()?;
                #[cfg(unix)]
                let name = Path::new(&endpoint)
                    .to_fs_name::<interprocess::local_socket::GenericFilePath>()?;
                let mut stream = Stream::connect(name)?;
                writeln!(stream, "{request}")?;
                stream.flush()?;
                let mut line = String::new();
                BufReader::new(stream).read_line(&mut line)?;
                Ok(serde_json::from_str(&line)?)
            })();
            let _ = tx.send(result.map_err(|err| err.to_string()));
        });
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|err| err.to_string())?
    }

    pub fn request(&self, request: Value) -> Value {
        self.try_request(request)
            .unwrap_or_else(|err| panic!("IPC: {err}\n{}", self.server_log()))
    }

    pub fn sessions(&self, resumable: bool) -> Vec<Value> {
        let result = self.request(json!({"op": "list", "resumable": resumable}));
        assert_eq!(result["status"], "ok", "{result}");
        result["sessions"].as_array().unwrap().clone()
    }

    pub fn notify(&self, provider: &str, payload: Value) {
        let mut command = self.berth();
        command
            .args(["notify", "--provider", provider])
            .stdin(Stdio::piped());
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        serde_json::to_writer(child.stdin.take().unwrap(), &payload).unwrap();
        let output = finish(child);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub fn hook(&self, provider: &str, sid: &str, event: &str, pid: Option<u32>) {
        let payload = json!({
            "session_id": sid, "hook_event_name": event,
            "cwd": self.project(), "pid": pid,
        });
        if pid.is_some() {
            self.notify(provider, payload);
        } else {
            // Synthetic sessions must not inherit a Codex process running the test suite.
            let response = self.request(json!({
                "op": "notify", "provider": provider, "payload": payload,
            }));
            assert_eq!(response["status"], "ok", "{response}");
        }
    }

    pub fn fixture_agent(&self) -> PathBuf {
        self.root.path().join("bin").join(executable("test-agent"))
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.stop();
        if std::thread::panicking() {
            self.root.disable_cleanup(true);
            eprintln!("Integration test artifacts: {}", self.root.path().display());
        }
    }
}

pub fn finish(child: Child) -> Output {
    finish_timeout(child, TIMEOUT)
}

pub fn finish_timeout(mut child: Child, timeout: Duration) -> Output {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "child timed out\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn success(command: &mut Command) -> Output {
    let output = finish(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert!(
        output.status.success(),
        "{:?} failed\n{}\n{}",
        command.get_program(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
