use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let exe = env::current_exe().unwrap();
    let name = exe.file_stem().unwrap().to_str().unwrap();
    if name == "hook-recorder" {
        let mut payload = String::new();
        io::stdin().read_to_string(&mut payload).unwrap();
        let berth = env::var_os("FIXTURE_BERTH_BIN").unwrap();
        let dir = PathBuf::from(env::var_os("FIXTURE_LOG_DIR").unwrap());
        let id = std::process::id();
        fs::write(dir.join(format!("{id}.payload")), &payload).unwrap();
        let mut child = command(&berth)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
        let output = command(&berth).args(["list", "--json"]).output().unwrap();
        assert!(output.status.success());
        fs::write(dir.join(format!("{id}.sessions")), output.stdout).unwrap();
    } else if name == "claude" && args == ["agents", "--json"] {
        println!("[]");
    } else if name == "tmux" {
        let mut log = args.join("\n");
        if args.iter().any(|arg| arg == "-C") {
            log.push_str("\n--stdin--\n");
            for line in io::stdin().lock().lines() {
                let line = line.unwrap();
                log.push_str(&line);
                log.push('\n');
                if line == "detach-client" {
                    break;
                }
            }
        }
        let dir = PathBuf::from(env::var_os("FIXTURE_LOG_DIR").unwrap());
        fs::write(dir.join(format!("{}.txt", std::process::id())), log).unwrap();
        if args.iter().any(|arg| arg == "has-session") {
            std::process::exit(1);
        }
    } else if name == "test-agent" {
        let report = format!(
            "{}\n{}\n{}\n{}",
            args.join("\n"),
            env::current_dir().unwrap().display(),
            env::var("AGENT_BERTH_SOCK").unwrap(),
            env::var("XDG_STATE_HOME").unwrap(),
        );
        let path = PathBuf::from(env::var_os("FIXTURE_AGENT_REPORT").unwrap());
        let pending = path.with_extension("pending");
        fs::write(&pending, report).unwrap();
        fs::rename(pending, path).unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    } else {
        panic!("unexpected fixture invocation: {name} {args:?}");
    }
}

fn command(program: &std::ffi::OsStr) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}
