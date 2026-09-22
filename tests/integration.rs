mod support;

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::mock_llm::MockLlm;
use support::{Sandbox, success};

/// Locate an agent client on PATH, honoring Windows shim extensions such as
/// the `.cmd` wrappers npm installs for JavaScript clients.
fn client_executable(name: &str, path: &OsStr) -> Option<PathBuf> {
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .map(|ext| ext.trim().to_ascii_lowercase())
            .filter(|ext| !ext.is_empty())
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(path).find_map(|dir| {
        extensions.iter().find_map(|ext| {
            let candidate = dir.join(format!("{name}{ext}"));
            candidate.is_file().then_some(candidate)
        })
    })
}

fn client_command(sandbox: &Sandbox, executable: &Path) -> std::process::Command {
    #[cfg(windows)]
    if executable
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
    {
        let mut command = sandbox.command("cmd.exe");
        command.arg("/c").arg(executable);
        return command;
    }
    sandbox.command(executable)
}

#[test]
fn hook_clients_report_lifecycle_over_ipc() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    for provider in ["claude", "codex", "grok"] {
        let sid = format!("{provider}-session");
        sandbox.hook(provider, &sid, "SessionStart", Some(std::process::id()));
        assert!(sandbox.sessions(false).is_empty());
        for (event, status) in [
            ("UserPromptSubmit", "running"),
            ("PermissionRequest", "waiting"),
            ("PostToolUse", "running"),
            ("Stop", "done"),
        ] {
            sandbox.hook(provider, &sid, event, Some(std::process::id()));
            let sessions = sandbox.sessions(false);
            assert_eq!(sessions.len(), 1, "{provider} {event}: {sessions:?}");
            assert_eq!(sessions[0]["provider"], provider);
            assert_eq!(sessions[0]["session_id"], sid);
            assert_eq!(sessions[0]["status"], status, "{provider} {event}");
            assert_eq!(sessions[0]["cwd"], sandbox.project().to_str().unwrap());
            assert_eq!(sessions[0]["pid"], std::process::id());
        }
        sandbox.hook(provider, &sid, "SessionEnd", Some(std::process::id()));
        assert!(sandbox.sessions(false).is_empty());
        assert!(sandbox.sessions(true).is_empty());
    }
}

#[test]
fn transcript_paths_roundtrip_through_notify_ipc_and_database() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    for provider in ["claude", "codex"] {
        let path = sandbox
            .root
            .path()
            .join(format!("{provider}-transcript.jsonl"));
        fs::write(&path, "").unwrap();
        sandbox.notify(
            provider,
            json!({
                "session_id":provider, "hook_event_name":"UserPromptSubmit",
                "pid": std::process::id(), "cwd":sandbox.project(), "transcript_path":path,
            }),
        );
        let listed = sandbox.sessions(false);
        let session = listed
            .iter()
            .find(|session| session["provider"] == provider)
            .unwrap();
        assert_eq!(session["transcript_path"], path.to_str().unwrap());
    }
    sandbox.stop();
    let output = success(sandbox.berth().args(["list", "--json"]));
    let offline: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(offline.len(), 2);
    assert!(
        offline
            .iter()
            .all(|session| session["transcript_path"].is_string())
    );
    sandbox.start();
    assert_eq!(sandbox.sessions(false).len(), 2);
    assert!(
        sandbox
            .sessions(false)
            .iter()
            .all(|session| session["transcript_path"].is_string())
    );
}

#[test]
fn codex_source_tracks_current_client_when_resuming_across_cli_and_desktop() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let sessions_dir = sandbox.root.path().join("codex/sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let transcript = sessions_dir.join("rollout-s.jsonl");
    for (originator, original_client, expected) in [
        (Some("Codex Desktop"), "codex-tui", "desktop"),
        (None, "Codex Desktop", "cli"),
        (Some("codex_work_desktop"), "codex-tui", "desktop"),
    ] {
        fs::write(
            &transcript,
            format!(
                "{}\n",
                json!({"type":"session_meta","payload":{
                    "id":"s", "originator":original_client
                }})
            ),
        )
        .unwrap();
        // Only the notify process receives the client's environment; the
        // already-running server must use the forwarded identity.
        if let Some(originator) = originator {
            sandbox.env.insert(
                "CODEX_INTERNAL_ORIGINATOR_OVERRIDE".into(),
                originator.into(),
            );
        } else {
            sandbox
                .env
                .remove(OsStr::new("CODEX_INTERNAL_ORIGINATOR_OVERRIDE"));
        }
        for event in ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"] {
            sandbox.notify(
                "codex",
                json!({
                    "session_id":"s", "hook_event_name":event,
                    "pid":std::process::id(), "transcript_path":transcript
                }),
            );
            let sessions = sandbox.sessions(false);
            assert_eq!(sessions.len(), 1, "{sessions:?}");
            assert_eq!(sessions[0]["source"], expected, "{event}");
        }
    }
    sandbox.stop();
    let output = success(sandbox.berth().args(["list", "--json"]));
    let offline: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(offline[0]["source"], "desktop");
}

#[test]
fn codex_titles_refresh_from_index_without_hook_events() {
    use std::io::Write;

    let mut sandbox = Sandbox::new();
    sandbox.start();
    sandbox.hook("codex", "s", "UserPromptSubmit", None);
    assert!(sandbox.sessions(false)[0]["title"].is_null());

    let mut index = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sandbox.root.path().join("codex/session_index.jsonl"))
        .unwrap();
    for title in ["Original title", "Manually renamed title"] {
        writeln!(
            index,
            "{}",
            json!({"id": "s", "thread_name": title, "updated_at": "2026-09-22T00:00:00Z"})
        )
        .unwrap();
        // Index titles reach the store through the server's heartbeat
        // discovery (every 5s), not synchronously on list.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let sessions = sandbox.sessions(false);
            if sessions[0]["title"] == title {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "title did not refresh from session index: {sessions:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    sandbox.stop();
    let offline = success(sandbox.berth().args(["list", "--json", "--resumable"]));
    let sessions: Vec<Value> = serde_json::from_slice(&offline.stdout).unwrap();
    assert_eq!(sessions[0]["title"], "Manually renamed title");
}

#[test]
fn plugin_clients_report_snapshots_and_blocking_over_ipc() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    for provider in ["opencode", "pi"] {
        sandbox.notify(
            provider,
            json!({
                "id": "fixture", "cwd": sandbox.project(),
                "status": {"busy-session": "busy", "idle-session": "idle"},
                "blocking": ["busy-session"],
                "titles": {"busy-session": "Fix the hook"},
            }),
        );
        let sessions = sandbox.sessions(false);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0]["status"], "waiting");
        assert_eq!(sessions[0]["title"], "Fix the hook");
        assert_eq!(sessions[1]["status"], "idle");
        sandbox.notify(
            provider,
            json!({
                "id": "fixture", "cwd": sandbox.project(),
                "status": {"busy-session": "busy"}, "blocking": [],
            }),
        );
        let sessions = sandbox.sessions(false);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0]["status"], "running");
        sandbox.notify(provider, json!({"id": "fixture", "status": {}}));
        assert!(sandbox.sessions(false).is_empty());
    }
}

#[test]
fn concurrent_servers_isolate_ipc_state_and_restarts() {
    let mut first = Sandbox::new();
    let mut second = Sandbox::new();
    first.start();
    second.start();
    assert_ne!(first.endpoint, second.endpoint);
    assert!(first.sessions(false).is_empty());
    assert!(second.sessions(false).is_empty());
    first.hook("codex", "first", "UserPromptSubmit", None);
    second.hook("codex", "second", "PermissionRequest", None);
    assert_eq!(first.sessions(false)[0]["session_id"], "first");
    assert_eq!(second.sessions(false)[0]["session_id"], "second");

    let duplicate = support::finish(
        first
            .berth()
            .arg("server")
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert!(!duplicate.status.success());
    first.stop();
    let offline = success(first.berth().args(["list", "--json", "--resumable"]));
    let sessions: Vec<Value> = serde_json::from_slice(&offline.stdout).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["session_id"], "first");
    first.start();
    let sessions = first.sessions(true);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["session_id"], "first");
    let sessions = second.sessions(false);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["session_id"], "second");
    for sandbox in [&first, &second] {
        assert!(
            sandbox
                .root
                .path()
                .join("state/agent-berth/state.redb")
                .is_file()
        );
        assert!(
            sandbox
                .root
                .path()
                .join("state/agent-berth/server.pid")
                .is_file()
        );
    }
}

#[test]
fn setup_installs_all_hooks_in_disposable_client_homes() {
    let sandbox = Sandbox::new();
    success(sandbox.berth().args(["setup", "--no-service"]));
    for (provider, file) in [
        ("claude", "claude/settings.json"),
        ("codex", "codex/hooks.json"),
        ("grok", "grok/hooks/agent-berth.json"),
        ("opencode", "config/opencode/plugins/agent-berth.js"),
        ("pi", "pi/extensions/agent-berth.ts"),
    ] {
        let contents = fs::read_to_string(sandbox.root.path().join(file)).unwrap();
        if matches!(provider, "claude" | "codex" | "grok") {
            let config: Value = serde_json::from_str(&contents).unwrap();
            let command = config["hooks"]["SessionStart"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap();
            let binary = command
                .strip_suffix(&format!(" notify --provider {provider}"))
                .unwrap()
                .trim_matches('"');
            assert_eq!(PathBuf::from(binary), PathBuf::from(support::BERTH));
        } else {
            let binary = contents
                .lines()
                .find_map(|line| line.strip_prefix("const BERTH_BIN = "))
                .unwrap();
            let binary: String = serde_json::from_str(binary).unwrap();
            assert_eq!(PathBuf::from(binary), PathBuf::from(support::BERTH));
            assert!(contents.contains(&format!("\"{provider}\"")));
        }
    }
    assert!(
        !sandbox
            .root
            .path()
            .join("config/systemd/user/agent-berth.service")
            .exists()
    );
}

#[test]
fn resume_passes_namespace_and_config_to_every_tmux_command() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    sandbox.hook("codex", "resume-session", "UserPromptSubmit", None);
    success(sandbox.berth().args(["resume", "--dry-run"]));
    assert_eq!(
        fs::read_dir(sandbox.root.path().join("tmux-log"))
            .unwrap()
            .count(),
        0
    );
    success(sandbox.berth().arg("resume"));
    let logs: Vec<_> = fs::read_dir(sandbox.root.path().join("tmux-log"))
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    assert_eq!(logs.len(), 4, "{logs:?}");
    let prefix = format!(
        "-u\n-L\n{}\n-f\n{}\n",
        sandbox.namespace,
        sandbox.root.path().join("tmux.conf").display()
    );
    assert!(logs.iter().all(|log| log.starts_with(&prefix)), "{logs:?}");
    assert!(logs.iter().any(
        |log| log.contains("\n-C\nattach\n-t\n=project\n--stdin--\n")
            && log.ends_with("detach-client\n")
    ));
    assert!(
        logs.iter()
            .any(|log| log.ends_with("\n--\ncodex\nresume\nresume-session"))
    );
}

#[test]
fn resume_pattern_matches_single_session() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    sandbox.hook("codex", "alpha", "UserPromptSubmit", None);
    sandbox.hook("codex", "beta", "UserPromptSubmit", None);
    let output = success(sandbox.berth().args(["resume", "--dry-run", "alpha"]));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("codex alpha"), "{text}");
    assert!(!text.contains("codex beta"), "{text}");
}

#[test]
fn list_resumable_here_filters_to_current_directory() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    sandbox.hook("codex", "here", "UserPromptSubmit", None);
    sandbox.notify(
        "opencode",
        json!({
            "id": "p", "cwd": sandbox.root.path(), "status": {"there": "busy"},
        }),
    );
    let output = success(
        sandbox
            .berth()
            .args(["list", "--json", "--resumable", "--here"]),
    );
    let sessions: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert_eq!(sessions[0]["session_id"], "here");
}

#[test]
fn rm_hides_selected_sessions_until_they_report_again() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    for sid in ["drop-a", "drop-b", "keep"] {
        sandbox.hook("claude", sid, "UserPromptSubmit", None);
    }
    sandbox.env.insert("FIXTURE_FZF_PICK".into(), "drop".into());
    success(sandbox.berth().arg("rm"));
    let sessions = sandbox.sessions(false);
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert_eq!(sessions[0]["session_id"], "keep");

    sandbox.hook("claude", "drop-a", "PermissionRequest", None);
    let sessions = sandbox.sessions(false);
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert!(
        sessions
            .iter()
            .any(|session| session["session_id"] == "drop-a"),
        "{sessions:?}"
    );
}

#[test]
fn codex_hook_without_pid_detects_agent_and_tmux_pane() {
    use std::io::Write;
    use std::process::Stdio;

    let mut sandbox = Sandbox::new();
    let mut paths: Vec<_> = std::env::split_paths(&sandbox.env[OsStr::new("PATH")]).collect();
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    sandbox
        .env
        .insert("PATH".into(), std::env::join_paths(paths).unwrap());
    sandbox
        .env
        .insert("FIXTURE_BERTH_BIN".into(), support::BERTH.into());
    sandbox.start();

    let mut agent = sandbox
        .command(
            sandbox
                .root
                .path()
                .join("bin")
                .join(format!("codex{}", std::env::consts::EXE_SUFFIX)),
        )
        .arg("test-hook")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = agent.id();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let payload = json!({
            "session_id": "codex-no-pid",
            "hook_event_name": "UserPromptSubmit",
            "cwd": sandbox.project(),
        });
        write!(agent.stdin.take().unwrap(), "{payload}").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !sandbox.root.path().join("agent-report.txt").exists() {
            assert!(agent.try_wait().unwrap().is_none(), "Codex fixture exited");
            assert!(Instant::now() < deadline, "Codex hook did not finish");
            std::thread::sleep(Duration::from_millis(25));
        }
        let sessions = sandbox.sessions(false);
        assert_eq!(sessions.len(), 1, "{sessions:?}");
        assert_eq!(sessions[0]["pid"], pid);

        sandbox.env.insert(
            "FIXTURE_TMUX_PANES".into(),
            format!(
                "%7\t{pid}\tproject\t0\tcodex\t{}",
                sandbox.project().display()
            )
            .into(),
        );
        let output = success(sandbox.berth().args(["attach", "--dry-run"]));
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.starts_with("%7\tcodex\trunning\t"), "{text}");
        assert!(text.contains("codex-no-pid"), "{text}");
    }));
    let _ = agent.kill();
    let _ = agent.wait();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[test]
fn attach_resolves_active_agent_pane_and_attaches() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let project = sandbox.project();
    fs::create_dir_all(project.join(".git")).unwrap();
    fs::write(project.join(".git/HEAD"), "ref: refs/heads/attach-work\n").unwrap();

    let mut agent = sandbox.command(sandbox.fixture_agent()).spawn().unwrap();
    let pid = agent.id();
    sandbox.notify(
        "pi",
        json!({
            "id": pid.to_string(),
            "cwd": project,
            "status": {"s1": "busy"},
            "titles": {"s1": "Fix attach command"},
        }),
    );
    sandbox.env.insert(
        "FIXTURE_TMUX_PANES".into(),
        format!("%7\t{pid}\tproject\t0\tshell\t{}", project.display()).into(),
    );

    let dry = success(sandbox.berth().args(["attach", "--dry-run"]));
    let text = String::from_utf8_lossy(&dry.stdout);
    assert!(text.contains("Fix attach command"), "{text}");
    assert!(text.contains("attach-work"), "{text}");
    assert!(text.contains("\tproject\t"), "{text}");

    sandbox
        .env
        .insert("FIXTURE_FZF_PICK".into(), "Fix attach".into());
    sandbox.notify(
        "pi",
        json!({
            "id": pid.to_string(),
            "cwd": project,
            "status": {"s1": "busy"},
            "titles": {"s1": "Fix attach command"},
        }),
    );
    success(sandbox.berth().arg("attach"));
    let logs: Vec<String> = fs::read_dir(sandbox.root.path().join("tmux-log"))
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    let joined = logs.join("\n---\n");
    assert!(joined.contains("select-window\n-t\n=project:0"), "{joined}");
    assert!(joined.contains("select-pane\n-t\n%7"), "{joined}");
    assert!(joined.contains("attach\n-t\n=project"), "{joined}");

    let _ = agent.kill();
    let _ = agent.wait();
}

#[test]
fn attach_session_scopes_panes_to_current_session() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    let mut agent = sandbox.command(sandbox.fixture_agent()).spawn().unwrap();
    let pid = agent.id();
    sandbox.notify(
        "pi",
        json!({
            "id": pid.to_string(),
            "cwd": sandbox.project(),
            "status": {"s1": "busy"},
        }),
    );
    sandbox.env.insert(
        "FIXTURE_TMUX_PANES".into(),
        format!(
            "%7\t{pid}\tproject\t0\tshell\t{}",
            sandbox.project().display()
        )
        .into(),
    );
    success(sandbox.berth().args(["attach", "--session", "--dry-run"]));
    let logs: Vec<String> = fs::read_dir(sandbox.root.path().join("tmux-log"))
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    assert!(
        logs.iter().any(|log| log.contains("list-panes\n-s")),
        "{logs:?}"
    );
    let _ = agent.kill();
    let _ = agent.wait();
}

struct RealTmux<'a> {
    sandbox: &'a Sandbox,
    executable: PathBuf,
}

impl RealTmux<'_> {
    fn command(&self) -> std::process::Command {
        let mut command = self.sandbox.command(&self.executable);
        command
            .args(["-L", &self.sandbox.namespace])
            .arg("-f")
            .arg(self.sandbox.root.path().join("tmux.conf"));
        command
    }
}

impl Drop for RealTmux<'_> {
    fn drop(&mut self) {
        let mut command = self.command();
        command
            .arg("kill-server")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Ok(mut child) = command.spawn() {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            eprintln!(
                                "tmux cleanup failed for {}: {status}",
                                self.sandbox.namespace
                            );
                        }
                        break;
                    }
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(25))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        eprintln!("tmux cleanup did not finish for {}", self.sandbox.namespace);
                        break;
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "requires real tmux/psmux; uses a disposable namespace and no model calls"]
fn real_tmux_resume_inherits_isolated_environment() {
    let original_path = std::env::var_os("PATH").unwrap();
    let executable = std::env::split_paths(&original_path)
        .map(|dir| dir.join(format!("tmux{}", std::env::consts::EXE_SUFFIX)))
        .find(|path| path.is_file())
        .expect("tmux/psmux must be installed");
    let mut sandbox = Sandbox::new();
    fs::remove_file(
        sandbox
            .root
            .path()
            .join("bin")
            .join(format!("tmux{}", std::env::consts::EXE_SUFFIX)),
    )
    .unwrap();
    let mut paths = vec![sandbox.root.path().join("bin")];
    paths.extend(std::env::split_paths(&original_path));
    sandbox
        .env
        .insert("PATH".into(), std::env::join_paths(paths).unwrap());
    let shell_config = if cfg!(windows) {
        "set -g default-shell powershell.exe\nset -g default-command 'powershell.exe -NoLogo -NoProfile'\n"
    } else {
        "set -g default-shell /bin/sh\nset -g default-command /bin/sh\n"
    };
    fs::write(sandbox.root.path().join("tmux.conf"), shell_config).unwrap();
    let mut neighbor = Sandbox::new();
    neighbor.env.insert(
        "PATH".into(),
        sandbox.env[std::ffi::OsStr::new("PATH")].clone(),
    );
    neighbor.env.insert(
        "PSMUX_DATA_DIR".into(),
        sandbox.env[std::ffi::OsStr::new("PSMUX_DATA_DIR")].clone(),
    );
    fs::write(neighbor.root.path().join("tmux.conf"), shell_config).unwrap();
    let neighbor_tmux = RealTmux {
        sandbox: &neighbor,
        executable: executable.clone(),
    };
    success(
        neighbor_tmux
            .command()
            .args(["new-session", "-d", "-s", "sentinel"]),
    );
    sandbox.start();
    sandbox.notify(
        "pi",
        json!({
            "id": "fixture", "cwd": sandbox.project(), "status": {"resume-session": "busy"},
            "cmdline": [sandbox.fixture_agent(), "--resume", "resume-session"],
        }),
    );
    let tmux = RealTmux {
        sandbox: &sandbox,
        executable,
    };
    success(sandbox.berth().arg("resume"));
    let report_path = sandbox.root.path().join("agent-report.txt");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !report_path.is_file() {
        assert!(
            Instant::now() < deadline,
            "resumed agent did not write its report"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let report = fs::read_to_string(report_path).unwrap();
    assert_eq!(
        report,
        format!(
            "--resume\nresume-session\n{}\n{}\n{}",
            sandbox.project().display(),
            sandbox.endpoint,
            sandbox.root.path().join("state").display()
        )
    );
    let panes = success(
        tmux.command()
            .args(["list-panes", "-a", "-F", "#{session_name}"]),
    );
    assert!(!panes.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&panes.stdout)
            .lines()
            .all(|name| name == "project")
    );
    drop(tmux);
    success(
        neighbor_tmux
            .command()
            .args(["has-session", "-t", "=sentinel"]),
    );
}

#[test]
#[ignore = "requires real tmux/psmux; uses a disposable namespace and no model calls"]
fn real_tmux_attach_resolves_agent_pane() {
    let original_path = std::env::var_os("PATH").unwrap();
    let executable = std::env::split_paths(&original_path)
        .map(|dir| dir.join(format!("tmux{}", std::env::consts::EXE_SUFFIX)))
        .find(|path| path.is_file())
        .expect("tmux/psmux must be installed");
    let mut sandbox = Sandbox::new();
    fs::remove_file(
        sandbox
            .root
            .path()
            .join("bin")
            .join(format!("tmux{}", std::env::consts::EXE_SUFFIX)),
    )
    .unwrap();
    let mut paths = vec![sandbox.root.path().join("bin")];
    paths.extend(std::env::split_paths(&original_path));
    sandbox
        .env
        .insert("PATH".into(), std::env::join_paths(paths).unwrap());
    let shell_config = if cfg!(windows) {
        "set -g default-shell powershell.exe\nset -g default-command 'powershell.exe -NoLogo -NoProfile'\n"
    } else {
        "set -g default-shell /bin/sh\nset -g default-command /bin/sh\n"
    };
    fs::write(sandbox.root.path().join("tmux.conf"), shell_config).unwrap();
    sandbox.start();

    let pid_file = sandbox.root.path().join("agent.pid");
    let script = sandbox.root.path().join(if cfg!(windows) {
        "agent.ps1"
    } else {
        "agent.sh"
    });
    if cfg!(windows) {
        fs::write(
            &script,
            format!(
                "$PID | Set-Content -Path '{}'\nStart-Sleep -Seconds 300\n",
                pid_file.display()
            ),
        )
        .unwrap();
    } else {
        fs::write(
            &script,
            format!("echo $$ > '{}'\nsleep 300\n", pid_file.display()),
        )
        .unwrap();
    }
    let command: Vec<String> = if cfg!(windows) {
        [
            "cmd.exe",
            "/c",
            "powershell.exe",
            "-NoLogo",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script.to_str().unwrap(),
        ]
        .iter()
        .map(|arg| arg.to_string())
        .collect()
    } else {
        vec!["sh".to_string(), script.display().to_string()]
    };

    let project = sandbox.project();
    let tmux = RealTmux {
        sandbox: &sandbox,
        executable,
    };
    success(tmux.command().args(["new-session", "-d", "-s", "agents"]));
    let mut new_window = tmux.command();
    new_window
        .arg("new-window")
        .arg("-t")
        .arg("=agents")
        .arg("-n")
        .arg("agent")
        .arg("-c")
        .arg(&project)
        .arg("--");
    for arg in &command {
        new_window.arg(arg);
    }
    success(&mut new_window);

    let deadline = Instant::now() + Duration::from_secs(15);
    while !pid_file.is_file() {
        assert!(
            Instant::now() < deadline,
            "pane command did not write its pid"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let pid: u32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    sandbox.notify(
        "pi",
        json!({
            "id": pid.to_string(),
            "cwd": project,
            "status": {"real": "busy"},
            "titles": {"real": "real-attach"},
        }),
    );

    let output = success(sandbox.berth().args(["attach", "--dry-run"]));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("real-attach"), "{text}");
    assert!(text.contains("project"), "{text}");
    drop(tmux);
}

fn install_recorder(sandbox: &mut Sandbox, provider: &str) -> PathBuf {
    let hook_path = match provider {
        "claude" => "claude/settings.json",
        "codex" => "codex/hooks.json",
        "grok" => "grok/hooks/agent-berth.json",
        "opencode" => "config/opencode/plugins/agent-berth.js",
        "pi" => "pi/extensions/agent-berth.ts",
        _ => unreachable!(),
    };
    let recorder = sandbox
        .root
        .path()
        .join("bin")
        .join(format!("hook-recorder{}", std::env::consts::EXE_SUFFIX));
    let hook_path = sandbox.root.path().join(hook_path);
    let contents = fs::read_to_string(&hook_path).unwrap();
    let binary = support::BERTH;
    let recorder_str = recorder.display().to_string();
    let mut replaced = None;
    for (from, to) in [
        (binary.to_string(), recorder_str.clone()),
        (
            binary.replace('\\', "\\\\"),
            recorder_str.replace('\\', "\\\\"),
        ),
        (binary.replace('\\', "/"), recorder_str.replace('\\', "/")),
    ] {
        if contents.contains(&from) {
            replaced = Some(contents.replace(&from, &to));
            break;
        }
    }
    let Some(contents) = replaced else {
        panic!("recorder was not installed")
    };
    fs::write(hook_path, contents).unwrap();
    sandbox
        .env
        .insert("FIXTURE_BERTH_BIN".into(), support::BERTH.into());
    recorder
}

#[test]
fn recording_hook_forwards_payload_and_observes_session() {
    let mut sandbox = Sandbox::new();
    success(sandbox.berth().args(["setup", "--no-service"]));
    let recorder = install_recorder(&mut sandbox, "codex");
    sandbox.start();
    let payload = json!({"session_id": "recorded", "hook_event_name": "UserPromptSubmit", "cwd": sandbox.project()});
    let mut child = sandbox
        .command(recorder)
        .args(["notify", "--provider", "codex"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), &payload).unwrap();
    assert!(support::finish(child).status.success());
    let records: Vec<_> = fs::read_dir(sandbox.root.path().join("tmux-log"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(records.len(), 2);
    let recorded = fs::read(
        records
            .iter()
            .find(|path| path.extension().unwrap() == "payload")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&recorded).unwrap(), payload);
    let sessions = fs::read(
        records
            .iter()
            .find(|path| path.extension().unwrap() == "sessions")
            .unwrap(),
    )
    .unwrap();
    let sessions: Vec<Value> = serde_json::from_slice(&sessions).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["session_id"], "recorded");
}

const TEST_PROMPT: &str = "Reply with exactly OK. Do not use tools.";

fn headless_args(provider: &str, prompt: &str) -> Vec<String> {
    match provider {
        "claude" => vec![
            "-p".into(),
            prompt.into(),
            "--output-format".into(),
            "json".into(),
        ],
        "codex" => vec![
            "exec".into(),
            "--json".into(),
            "--skip-git-repo-check".into(),
            "--dangerously-bypass-hook-trust".into(),
            prompt.into(),
        ],
        "grok" => vec![
            "--single".into(),
            prompt.into(),
            "--output-format".into(),
            "json".into(),
        ],
        "opencode" => vec![
            "run".into(),
            "--format".into(),
            "json".into(),
            prompt.into(),
        ],
        "pi" => vec![
            "--print".into(),
            "--mode".into(),
            "json".into(),
            prompt.into(),
        ],
        _ => unreachable!(),
    }
}

#[test]
#[ignore = "requires AGENT_BERTH_TEST_CLIENT and credentials; makes a real model call"]
fn live_client_emits_hooks_and_reaches_berth() {
    let provider = std::env::var("AGENT_BERTH_TEST_CLIENT")
        .expect("set AGENT_BERTH_TEST_CLIENT to claude, codex, grok, opencode, or pi");
    assert!(["claude", "codex", "grok", "opencode", "pi"].contains(&provider.as_str()));
    let original_path = std::env::var_os("PATH").unwrap();
    let executable =
        client_executable(&provider, &original_path).expect("selected client must be installed");
    let mut sandbox = Sandbox::new();
    success(sandbox.berth().args(["setup", "--no-service"]));
    install_recorder(&mut sandbox, &provider);
    fs::remove_file(
        sandbox
            .root
            .path()
            .join("bin")
            .join(format!("{provider}{}", std::env::consts::EXE_SUFFIX)),
    )
    .unwrap();
    let mut paths = vec![sandbox.root.path().join("bin")];
    paths.extend(std::env::split_paths(&original_path));
    sandbox
        .env
        .insert("PATH".into(), std::env::join_paths(paths).unwrap());
    sandbox.start();

    let mut command = client_command(&sandbox, &executable);
    for key in [
        "ANTHROPIC_API_KEY",
        "CODEX_API_KEY",
        "OPENAI_API_KEY",
        "XAI_API_KEY",
        "GROK_API_KEY",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.args(headless_args(&provider, TEST_PROMPT));
    if let Ok(model) = std::env::var("AGENT_BERTH_TEST_MODEL") {
        command.args(["--model", &model]);
    }
    let stdout = fs::File::create(sandbox.root.path().join("client.stdout")).unwrap();
    let stderr = fs::File::create(sandbox.root.path().join("client.stderr")).unwrap();
    let output = support::finish_timeout(
        command
            .stdin(std::process::Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .unwrap(),
        Duration::from_secs(120),
    );
    assert!(
        output.status.success(),
        "client failed; see client.stderr in the retained artifacts"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let observed = fs::read_dir(sandbox.root.path().join("tmux-log"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "sessions")
            })
            .filter_map(|entry| fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<Vec<Value>>(&bytes).ok())
            .any(|sessions| {
                sessions
                    .iter()
                    .any(|session| session["provider"] == provider)
            });
        if observed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no client-generated session reached the berth; inspect recorded payloads"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn configure_mock_provider(sandbox: &mut Sandbox, provider: &str, mock: &MockLlm) {
    match provider {
        "claude" => {
            sandbox
                .env
                .insert("ANTHROPIC_BASE_URL".into(), mock.url().into());
            sandbox
                .env
                .insert("ANTHROPIC_AUTH_TOKEN".into(), "mock".into());
            sandbox
                .env
                .insert("ANTHROPIC_MODEL".into(), "mock-model".into());
            sandbox.env.insert(
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
                "1".into(),
            );
            let claude_json = json!({
                "hasCompletedOnboarding": true,
                "projects": {
                    sandbox.project().to_str().unwrap(): {"hasTrustDialogAccepted": true},
                },
            });
            fs::write(
                sandbox.root.path().join("home/.claude.json"),
                claude_json.to_string(),
            )
            .unwrap();
        }
        "codex" => {
            let config = format!(
                "model = \"mock-model\"\nmodel_provider = \"mock\"\n\n[model_providers.mock]\nname = \"Mock\"\nbase_url = \"{}/v1\"\nwire_api = \"responses\"\nenv_key = \"MOCK_LLM_API_KEY\"\n",
                mock.url()
            );
            fs::write(sandbox.root.path().join("codex/config.toml"), config).unwrap();
            sandbox.env.insert("MOCK_LLM_API_KEY".into(), "mock".into());
        }
        "grok" => {
            for (key, value) in [
                ("GROK_XAI_API_BASE_URL", format!("{}/v1", mock.url())),
                ("GROK_MODELS_BASE_URL", format!("{}/v1", mock.url())),
                ("GROK_MODELS_LIST_URL", format!("{}/v1/models", mock.url())),
                ("XAI_API_KEY", "mock".to_string()),
                ("GROK_CODE_XAI_API_KEY", "mock".to_string()),
                ("GROK_DEFAULT_MODEL", "mock-model".to_string()),
                ("GROK_TELEMETRY_ENABLED", "0".to_string()),
                ("GROK_DISABLE_AUTOUPDATER", "1".to_string()),
            ] {
                sandbox.env.insert(key.into(), value.into());
            }
        }
        "opencode" => {
            let config = json!({
                "$schema": "https://opencode.ai/config.json",
                "model": "mock/mock-model",
                "provider": {
                    "mock": {
                        "npm": "@ai-sdk/openai-compatible",
                        "name": "Mock",
                        "options": {"baseURL": format!("{}/v1", mock.url()), "apiKey": "mock"},
                        "models": {"mock-model": {"name": "Mock Model"}},
                    },
                },
            });
            let path = sandbox.root.path().join("config/opencode/opencode.json");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, config.to_string()).unwrap();
        }
        "pi" => {
            let models = json!({
                "providers": {
                    "mock": {
                        "baseUrl": format!("{}/v1", mock.url()),
                        "apiKey": "mock",
                        "api": "openai-completions",
                        "models": [{
                            "id": "mock-model",
                            "name": "Mock Model",
                            "reasoning": false,
                            "input": ["text"],
                            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
                            "contextWindow": 128000,
                            "maxTokens": 4096,
                        }],
                    },
                },
            });
            fs::write(
                sandbox.root.path().join("pi/models.json"),
                models.to_string(),
            )
            .unwrap();
        }
        _ => unreachable!(),
    }
}

#[test]
#[ignore = "requires AGENT_BERTH_TEST_CLIENT and the installed client; calls a local mock LLM only"]
fn mock_llm_client_completes_and_reports_terminal_state() {
    let provider = std::env::var("AGENT_BERTH_TEST_CLIENT")
        .expect("set AGENT_BERTH_TEST_CLIENT to claude, codex, grok, opencode, or pi");
    assert!(["claude", "codex", "grok", "opencode", "pi"].contains(&provider.as_str()));
    let original_path = std::env::var_os("PATH").unwrap();
    let executable =
        client_executable(&provider, &original_path).expect("selected client must be installed");
    let mut sandbox = Sandbox::new();
    success(sandbox.berth().args(["setup", "--no-service"]));
    install_recorder(&mut sandbox, &provider);
    fs::remove_file(
        sandbox
            .root
            .path()
            .join("bin")
            .join(format!("{provider}{}", std::env::consts::EXE_SUFFIX)),
    )
    .unwrap();
    let mut paths = vec![sandbox.root.path().join("bin")];
    paths.extend(std::env::split_paths(&original_path));
    sandbox
        .env
        .insert("PATH".into(), std::env::join_paths(paths).unwrap());
    let plugin = matches!(provider.as_str(), "opencode" | "pi");
    // Keep the conversation open briefly so async hooks and plugin heartbeats
    // flush before the client exits.
    let mock = MockLlm::start_with_delay(if plugin {
        Duration::from_secs(3)
    } else {
        Duration::from_secs(2)
    });
    configure_mock_provider(&mut sandbox, &provider, &mock);
    sandbox.start();

    let mut command = client_command(&sandbox, &executable);
    command.args(headless_args(&provider, TEST_PROMPT));
    match provider.as_str() {
        "claude" => {
            // --debug keeps hook execution logs in the retained artifacts.
            command.arg("--debug");
        }
        "grok" => {
            command.args(["--model", "mock-model"]);
        }
        "opencode" | "pi" => {
            command.args(["--model", "mock/mock-model"]);
        }
        _ => {}
    }
    let stdout = fs::File::create(sandbox.root.path().join("client.stdout")).unwrap();
    let stderr = fs::File::create(sandbox.root.path().join("client.stderr")).unwrap();
    let output = support::finish_timeout(
        command
            .stdin(std::process::Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .unwrap(),
        // opencode downloads its provider package into the fresh sandbox cache
        // on every run, which can take over a minute on a slow network.
        Duration::from_secs(240),
    );
    assert!(
        output.status.success(),
        "client failed; see client.stderr in the retained artifacts"
    );
    assert!(
        mock.completions() > 0,
        "client made no completion request to the mock LLM; requests: {:?}",
        mock.requests()
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let observed = fs::read_dir(sandbox.root.path().join("tmux-log"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "sessions")
            })
            .filter_map(|entry| fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<Vec<Value>>(&bytes).ok())
            .any(|sessions| {
                sessions
                    .iter()
                    .any(|session| session["provider"] == provider)
            });
        if observed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no client-generated session reached the berth; inspect recorded payloads"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // The mock ends the conversation on the first response, so the terminal
    // state is deterministic: hook clients send SessionEnd, and plugin client
    // snapshots go idle or leave the active list once the process exits.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let settled = if plugin {
            sandbox
                .sessions(false)
                .iter()
                .filter(|session| session["provider"] == provider)
                .all(|session| session["status"] == "idle")
        } else {
            // claude does not emit SessionEnd in print mode; its terminal hook
            // is Stop. codex and grok emit SessionEnd on exit.
            let terminal_hook = if provider == "claude" {
                "Stop"
            } else {
                "SessionEnd"
            };
            let ended = fs::read_dir(sandbox.root.path().join("tmux-log"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "payload"))
                .filter_map(|entry| fs::read_to_string(entry.path()).ok())
                .any(|payload| payload.contains(terminal_hook));
            ended
                && !sandbox
                    .sessions(false)
                    .iter()
                    .any(|session| session["provider"] == provider)
        };
        if settled {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "client session did not reach its terminal state: {:?}",
            sandbox.sessions(false)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
