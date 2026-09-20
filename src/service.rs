use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::ipc;
use crate::paths::Context as AppContext;

#[cfg(windows)]
const TASK_NAME: &str = "AgentBerth";

const START_TIMEOUT: Duration = Duration::from_secs(15);

pub fn server_log_path(ctx: &AppContext) -> std::path::PathBuf {
    ctx.state_dir.join("server.log")
}

pub fn install(ctx: &AppContext) -> Result<()> {
    if install_backend(ctx)? {
        wait_running(ctx)?;
    }
    Ok(())
}

pub fn uninstall(ctx: &AppContext) -> Result<()> {
    remove_backend(ctx);
    cleanup(ctx);
    Ok(())
}

pub fn start(ctx: &AppContext) -> Result<()> {
    if running(ctx) {
        return Ok(());
    }
    ensure_installed(ctx)?;
    start_backend(ctx)?;
    wait_running(ctx)
}

pub fn stop(ctx: &AppContext) -> Result<()> {
    stop_backend(ctx);
    cleanup(ctx);
    Ok(())
}

pub fn restart(ctx: &AppContext) -> Result<()> {
    ensure_installed(ctx)?;
    restart_backend(ctx)?;
    wait_running(ctx)
}

pub fn running(ctx: &AppContext) -> bool {
    ipc::ping(ctx).is_ok()
}

pub fn installed(ctx: &AppContext) -> bool {
    #[cfg(windows)]
    {
        let _ = ctx;
        Windows::task_exists()
    }
    #[cfg(unix)]
    {
        ctx.systemd_unit_path().is_file()
    }
}

fn ensure_installed(ctx: &AppContext) -> Result<()> {
    if installed(ctx) {
        Ok(())
    } else {
        anyhow::bail!("agent-berth service is not installed (run: agent-berth setup)")
    }
}

fn wait_running(ctx: &AppContext) -> Result<()> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if running(ctx) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "agent-berth server did not start; see {}",
                server_log_path(ctx).display()
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn cleanup(ctx: &AppContext) {
    if let Ok(pid_text) = std::fs::read_to_string(ctx.pid_path()) {
        if let Ok(pid) = pid_text.trim().parse::<u32>() {
            kill_pid(pid);
        }
    }
    let _ = std::fs::remove_file(ctx.pid_path());
    #[cfg(unix)]
    let _ = std::fs::remove_file(ctx.socket_path());
}

#[cfg(windows)]
fn install_backend(ctx: &AppContext) -> Result<bool> {
    Windows::run(ctx, "install")?;
    println!("Installed Windows scheduled task {TASK_NAME}");
    Ok(true)
}

#[cfg(windows)]
fn start_backend(ctx: &AppContext) -> Result<()> {
    Windows::run(ctx, "start")
}

#[cfg(windows)]
fn stop_backend(ctx: &AppContext) {
    if installed(ctx) {
        let _ = Windows::run(ctx, "stop");
    }
}

#[cfg(windows)]
fn restart_backend(ctx: &AppContext) -> Result<()> {
    Windows::run(ctx, "restart")
}

#[cfg(windows)]
fn remove_backend(ctx: &AppContext) {
    let _ = Windows::run(ctx, "uninstall");
    println!("Removed Windows scheduled task {TASK_NAME}");
}

#[cfg(unix)]
fn install_backend(ctx: &AppContext) -> Result<bool> {
    install_systemd(ctx)
}

#[cfg(unix)]
fn start_backend(_ctx: &AppContext) -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "start", "agent-berth.service"])
        .status()
        .context("systemctl start")?;
    if !status.success() {
        anyhow::bail!("failed to start agent-berth.service");
    }
    Ok(())
}

#[cfg(unix)]
fn stop_backend(_ctx: &AppContext) {
    let _ = Command::new("systemctl")
        .args(["--user", "stop", "agent-berth.service"])
        .status();
}

#[cfg(unix)]
fn restart_backend(_ctx: &AppContext) -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "restart", "agent-berth.service"])
        .status()
        .context("systemctl restart")?;
    if !status.success() {
        anyhow::bail!("failed to restart agent-berth.service");
    }
    Ok(())
}

#[cfg(unix)]
fn remove_backend(ctx: &AppContext) {
    uninstall_systemd(ctx);
}

#[cfg(windows)]
struct Windows;

#[cfg(windows)]
impl Windows {
    fn task_exists() -> bool {
        Command::new("schtasks")
            .args(["/Query", "/TN", TASK_NAME])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn run(ctx: &AppContext, action: &str) -> Result<()> {
        let script = Self::script(ctx);
        let path =
            std::env::temp_dir().join(format!("agent-berth-service-{}.ps1", std::process::id()));
        let mut content = vec![0xEF, 0xBB, 0xBF];
        content.extend_from_slice(script.as_bytes());
        std::fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
        let status = Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&path)
            .arg(action)
            .status();
        let _ = std::fs::remove_file(&path);
        let status = status.context("run powershell")?;
        if !status.success() {
            anyhow::bail!("agent-berth service {action} failed");
        }
        Ok(())
    }

    fn script(ctx: &AppContext) -> String {
        let exe = ctx.berth_bin.display().to_string().replace('\'', "''");
        let pipe = ctx.pipe_name().replace('\'', "''");
        format!(
            r#"param([Parameter(Position = 0)][ValidateSet('install', 'uninstall', 'start', 'stop', 'restart')][string]$Action = 'start')

$ErrorActionPreference = 'Stop'
$TaskName = '{TASK_NAME}'
$Exe = '{exe}'
$PipeName = '{pipe}'

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]$identity
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {{
    $shell = if ($PSVersionTable.PSEdition -eq 'Core') {{ 'pwsh' }} else {{ 'powershell' }}
    $relaunch = @('-NoProfile', '-WindowStyle', 'Hidden', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", $Action)
    $proc = Start-Process -Verb RunAs -FilePath $shell -ArgumentList $relaunch -Wait -PassThru
    exit $proc.ExitCode
}}

function Test-AgentBerthTask {{
    schtasks /Query /TN $TaskName *> $null
    return $LASTEXITCODE -eq 0
}}

function Wait-AgentBerthStopped {{
    for ($i = 0; $i -lt 50; $i++) {{
        if (-not (Test-Path "\\.\pipe\$PipeName")) {{ return }}
        Start-Sleep -Milliseconds 200
    }}
}}

function Register-AgentBerthTask {{
    $user = $identity.Name
    $xml = @"
<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Agent Berth server</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>$user</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>$user</UserId>
      <LogonType>S4U</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>$Exe</Command>
      <Arguments>server</Arguments>
    </Exec>
  </Actions>
</Task>
"@
    $tmp = [IO.Path]::GetTempFileName()
    try {{
        [IO.File]::WriteAllText($tmp, $xml, [Text.Encoding]::Unicode)
        schtasks /Create /TN $TaskName /XML $tmp /F | Out-Null
        if ($LASTEXITCODE -ne 0) {{ throw "failed to register scheduled task '$TaskName'" }}
    }} finally {{
        Remove-Item $tmp -Force -ErrorAction SilentlyContinue
    }}
}}

switch ($Action) {{
    'install' {{
        Register-AgentBerthTask
        schtasks /End /TN $TaskName *> $null
        Wait-AgentBerthStopped
        schtasks /Run /TN $TaskName | Out-Null
    }}
    'start' {{
        if (-not (Test-AgentBerthTask)) {{ throw "scheduled task '$TaskName' is not installed; run: agent-berth setup" }}
        schtasks /Run /TN $TaskName | Out-Null
    }}
    'stop' {{
        if (Test-AgentBerthTask) {{ schtasks /End /TN $TaskName | Out-Null }}
    }}
    'restart' {{
        if (Test-AgentBerthTask) {{ schtasks /End /TN $TaskName | Out-Null }}
        Wait-AgentBerthStopped
        schtasks /Run /TN $TaskName | Out-Null
    }}
    'uninstall' {{
        if (Test-AgentBerthTask) {{
            schtasks /End /TN $TaskName *> $null
            schtasks /Delete /TN $TaskName /F | Out-Null
        }}
    }}
}}
"#
        )
    }
}

#[cfg(unix)]
fn install_systemd(ctx: &AppContext) -> Result<bool> {
    let unit = ctx.systemd_unit_path();
    if let Some(parent) = unit.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let exec = crate::paths::quote_path(&ctx.berth_bin);
    let body = format!(
        "[Unit]\nDescription=Agent Berth\nAfter=default.target\n\n[Service]\nType=simple\nExecStart={exec} server\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n"
    );
    std::fs::write(&unit, body)?;
    let daemon = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    match daemon {
        Ok(status) if status.success() => {
            let enable = Command::new("systemctl")
                .args(["--user", "enable", "--now", "agent-berth.service"])
                .status()
                .context("systemctl enable")?;
            if !enable.success() {
                anyhow::bail!("failed to enable agent-berth.service");
            }
            println!("Installed systemd user unit {}", unit.display());
            return Ok(true);
        }
        _ => {
            println!(
                "Wrote {} (systemctl --user not available; enable it later)",
                unit.display()
            );
        }
    }
    Ok(false)
}

#[cfg(unix)]
fn uninstall_systemd(ctx: &AppContext) {
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "agent-berth.service"])
        .status();
    let unit = ctx.systemd_unit_path();
    if unit.exists() {
        let _ = std::fs::remove_file(&unit);
    }
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    println!("Removed systemd user unit");
}

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
}
