use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail, ensure};

use crate::paths::{self, Context};

pub(super) fn open(ctx: &Context, url: &str) -> Result<()> {
    let app = find(ctx)?;
    let mut command =
        if cfg!(target_os = "macos") && app.extension().is_some_and(|ext| ext == "app") {
            let mut command = Command::new("open");
            command.args(["-n", "-a"]).arg(&app).arg("--args");
            command
        } else {
            Command::new(&app)
        };
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("ELECTRON_RUN_AS_NODE")
        .env_remove("ELECTRON_NO_ATTACH_CONSOLE")
        .env_remove("PASEO_NODE_ENV")
        .env_remove("PASEO_DESKTOP_CLI");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("launch Paseo Desktop: {}", app.display()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn find(ctx: &Context) -> Result<PathBuf> {
    if let Some(path) = paths::env_path("PASEO_DESKTOP_BIN") {
        ensure!(
            path.is_file()
                || cfg!(target_os = "macos")
                    && path.extension().is_some_and(|ext| ext == "app")
                    && path.is_dir(),
            "PASEO_DESKTOP_BIN does not point to Paseo Desktop: {}",
            path.display()
        );
        return Ok(path);
    }
    let mut candidates = Vec::new();
    if cfg!(windows) {
        let scoop = paths::env_path("SCOOP").unwrap_or_else(|| ctx.home.join("scoop"));
        candidates.push(scoop.join("apps/paseo/current/Paseo.exe"));
        if let Some(local) = paths::env_path("LOCALAPPDATA") {
            candidates.push(local.join("Programs/Paseo/Paseo.exe"));
        }
        if let Some(programs) = paths::env_path("ProgramFiles") {
            candidates.push(programs.join("Paseo/Paseo.exe"));
        }
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("Paseo.exe")));
        }
    } else if cfg!(target_os = "macos") {
        candidates.extend([
            PathBuf::from("/Applications/Paseo.app"),
            ctx.home.join("Applications/Paseo.app"),
        ]);
    } else {
        if let Some(binary) = paths::find_executable("Paseo") {
            candidates.push(binary);
        }
        candidates.extend([
            PathBuf::from("/usr/bin/Paseo"),
            PathBuf::from("/opt/Paseo/Paseo"),
            ctx.home.join("Applications/Paseo.AppImage"),
        ]);
    }
    if let Some(path) = first_app(&candidates) {
        return Ok(path);
    }
    bail!("Paseo Desktop executable not found; set PASEO_DESKTOP_BIN to its executable path")
}

fn first_app(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|path| path.is_file() || is_app_bundle(path))
        .cloned()
}

fn is_app_bundle(path: &Path) -> bool {
    cfg!(target_os = "macos") && path.extension().is_some_and(|ext| ext == "app") && path.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_scoop_and_linux_desktop_executables_without_cli_shims() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("scoop/apps/paseo/current");
        std::fs::create_dir_all(bin.join("resources/bin")).unwrap();
        std::fs::write(bin.join("resources/bin/paseo.cmd"), "CLI shim").unwrap();
        let exe = bin.join("Paseo.exe");
        let linux = root.path().join("Paseo.AppImage");
        let candidates = [exe.clone(), linux.clone()];
        assert!(first_app(&candidates).is_none());
        std::fs::write(&linux, "desktop").unwrap();
        assert_eq!(first_app(&candidates), Some(linux));
        std::fs::write(&exe, "desktop").unwrap();
        assert_eq!(first_app(&candidates), Some(exe));
    }
}
