use std::process::Command;

use anyhow::{Context, Result};

use crate::ipc;
use crate::paths::Context as AppContext;

#[cfg(windows)]
const TASK_NAME: &str = "AgentBerth";

pub fn install(ctx: &AppContext) -> Result<()> {
    #[cfg(windows)]
    {
        install_windows(ctx)
    }
    #[cfg(unix)]
    {
        install_systemd(ctx)
    }
}

pub fn uninstall(ctx: &AppContext) -> Result<()> {
    stop(ctx)?;
    #[cfg(windows)]
    {
        uninstall_windows()
    }
    #[cfg(unix)]
    {
        uninstall_systemd(ctx)
    }
}

pub fn stop(ctx: &AppContext) -> Result<()> {
    #[cfg(windows)]
    {
        let _ = Command::new("schtasks")
            .args(["/End", "/TN", TASK_NAME])
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", "agent-berth.service"])
            .status();
    }
    if let Ok(pid_text) = std::fs::read_to_string(ctx.pid_path()) {
        if let Ok(pid) = pid_text.trim().parse::<u32>() {
            kill_pid(pid);
        }
    }
    let _ = std::fs::remove_file(ctx.pid_path());
    #[cfg(unix)]
    let _ = std::fs::remove_file(ctx.socket_path());
    Ok(())
}

pub fn running(ctx: &AppContext) -> bool {
    ipc::ping(ctx).is_ok()
}

pub fn installed(ctx: &AppContext) -> bool {
    #[cfg(windows)]
    {
        let _ = ctx;
        std::process::Command::new("schtasks")
            .args(["/Query", "/TN", TASK_NAME])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(unix)]
    {
        ctx.systemd_unit_path().is_file()
    }
}

#[cfg(windows)]
fn install_windows(ctx: &AppContext) -> Result<()> {
    let exe = ctx.bridge_bin.display().to_string().replace('\'', "''");
    let script = format!(
        r#"
$exe = '{exe}'
$action = New-ScheduledTaskAction -Execute $exe -Argument 'server'
$trigger = New-ScheduledTaskTrigger -AtLogOn
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero)
Register-ScheduledTask -TaskName '{TASK_NAME}' -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName '{TASK_NAME}'
"#
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()
        .context("powershell")?;
    if !status.success() {
        anyhow::bail!("failed to install Windows scheduled task {TASK_NAME}");
    }
    println!("Installed Windows scheduled task {TASK_NAME}");
    Ok(())
}

#[cfg(windows)]
fn uninstall_windows() -> Result<()> {
    let _ = Command::new("schtasks")
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .status();
    println!("Removed Windows scheduled task {TASK_NAME}");
    Ok(())
}

#[cfg(unix)]
fn install_systemd(ctx: &AppContext) -> Result<()> {
    let unit = ctx.systemd_unit_path();
    if let Some(parent) = unit.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let exec = crate::paths::quote_path(&ctx.bridge_bin);
    let body = format!(
        "[Unit]\nDescription=Agent Bridge\nAfter=default.target\n\n[Service]\nType=simple\nExecStart={exec} server\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n"
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
        }
        _ => {
            println!(
                "Wrote {} (systemctl --user not available; enable it later)",
                unit.display()
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
fn uninstall_systemd(ctx: &AppContext) -> Result<()> {
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "agent-berth.service"])
        .status();
    let unit = ctx.systemd_unit_path();
    if unit.exists() {
        std::fs::remove_file(&unit)?;
    }
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    println!("Removed systemd user unit");
    Ok(())
}

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status();
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
}
