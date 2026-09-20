mod support;

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{Sandbox, success};

#[test]
fn hook_clients_report_lifecycle_over_ipc() {
    let mut sandbox = Sandbox::new();
    sandbox.start();
    for provider in ["claude", "codex", "grok"] {
        let sid = format!("{provider}-session");
        sandbox.hook(provider, &sid, "SessionStart", Some(std::process::id()));
        assert!(sandbox.sessions(false).is_empty());
        for (event, status) in [
            ("UserPromptSubmit", "working"),
            ("PermissionRequest", "waiting"),
            ("PostToolUse", "working"),
            ("Stop", "done"),
        ] {
            sandbox.hook(provider, &sid, event, Some(std::process::id()));
            let sessions = sandbox.sessions(false);
            assert_eq!(sessions.len(), 1, "{provider} {event}: {sessions:?}");
            assert_eq!(sessions[0]["provider"], provider);
            assert_eq!(sessions[0]["session_id"], sid);
            assert_eq!(sessions[0]["status"], status, "{provider} {event}");
            assert_eq!(sessions[0]["cwd"], sandbox.project().to_str().unwrap());
        }
        sandbox.hook(provider, &sid, "SessionEnd", Some(std::process::id()));
        assert!(sandbox.sessions(false).is_empty());
        assert!(sandbox.sessions(true).is_empty());
    }
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
            }),
        );
        let sessions = sandbox.sessions(false);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0]["status"], "waiting");
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
        assert_eq!(sessions[0]["status"], "working");
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
            .bridge()
            .arg("server")
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    assert!(!duplicate.status.success());
    first.stop();
    let offline = success(first.bridge().args(["list", "--json", "--resumable"]));
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
    success(sandbox.bridge().args(["setup", "--no-service"]));
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
            assert_eq!(PathBuf::from(binary), PathBuf::from(support::BRIDGE));
        } else {
            let binary = contents
                .lines()
                .find_map(|line| line.strip_prefix("const BRIDGE_BIN = "))
                .unwrap();
            let binary: String = serde_json::from_str(binary).unwrap();
            assert_eq!(PathBuf::from(binary), PathBuf::from(support::BRIDGE));
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
    success(sandbox.bridge().args(["resume", "--dry-run"]));
    assert_eq!(
        fs::read_dir(sandbox.root.path().join("tmux-log"))
            .unwrap()
            .count(),
        0
    );
    success(sandbox.bridge().arg("resume"));
    let logs: Vec<_> = fs::read_dir(sandbox.root.path().join("tmux-log"))
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    assert_eq!(logs.len(), 4, "{logs:?}");
    let prefix = format!(
        "-L\n{}\n-f\n{}\n",
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
    success(sandbox.bridge().arg("resume"));
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
    let binary = support::BRIDGE;
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
        .insert("FIXTURE_BRIDGE_BIN".into(), support::BRIDGE.into());
    recorder
}

#[test]
fn recording_hook_forwards_payload_and_observes_session() {
    let mut sandbox = Sandbox::new();
    success(sandbox.bridge().args(["setup", "--no-service"]));
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

#[test]
#[ignore = "requires AGENT_BERTH_TEST_CLIENT and credentials; makes a real model call"]
fn live_client_emits_hooks_and_reaches_bridge() {
    let provider = std::env::var("AGENT_BERTH_TEST_CLIENT")
        .expect("set AGENT_BERTH_TEST_CLIENT to claude, codex, grok, opencode, or pi");
    assert!(["claude", "codex", "grok", "opencode", "pi"].contains(&provider.as_str()));
    let original_path = std::env::var_os("PATH").unwrap();
    let executable = std::env::split_paths(&original_path)
        .map(|dir| dir.join(format!("{provider}{}", std::env::consts::EXE_SUFFIX)))
        .find(|path| path.is_file())
        .expect("selected client must be installed");
    let mut sandbox = Sandbox::new();
    success(sandbox.bridge().args(["setup", "--no-service"]));
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

    let mut command = sandbox.command(executable);
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
    let prompt = "Reply with exactly OK. Do not use tools.";
    match provider.as_str() {
        "claude" => {
            command.args(["-p", prompt, "--output-format", "json"]);
        }
        "codex" => {
            command.args([
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--dangerously-bypass-hook-trust",
                prompt,
            ]);
        }
        "grok" => {
            command.args(["--single", prompt, "--output-format", "json"]);
        }
        "opencode" => {
            command.args(["run", "--format", "json", prompt]);
        }
        "pi" => {
            command.args(["--print", "--mode", "json", prompt]);
        }
        _ => unreachable!(),
    }
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
            "no client-generated session reached the bridge; inspect recorded payloads"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
