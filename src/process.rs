pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        unix_alive(pid)
    }
    #[cfg(windows)]
    {
        windows_alive(pid)
    }
}

/// Process ids from `pid` up to its root ancestor, starting with `pid`.
pub fn ancestors(pid: u32) -> Vec<u32> {
    if pid == 0 {
        return Vec::new();
    }
    #[cfg(unix)]
    {
        unix_ancestors(pid)
    }
    #[cfg(windows)]
    {
        windows_ancestors(pid)
    }
}

fn walk_ancestors(pid: u32, parents: &std::collections::HashMap<u32, u32>) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut current = pid;
    while chain.len() < 256 {
        let Some(&parent) = parents.get(&current) else {
            break;
        };
        if parent == 0 || chain.contains(&parent) {
            break;
        }
        chain.push(parent);
        current = parent;
    }
    chain
}

#[cfg(unix)]
fn unix_ancestors(pid: u32) -> Vec<u32> {
    let output = std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid="])
        .output();
    let Ok(output) = output else {
        return vec![pid];
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parents = std::collections::HashMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(child), Some(parent)) = (fields.next(), fields.next()) else {
            continue;
        };
        if let (Ok(child), Ok(parent)) = (child.parse::<u32>(), parent.parse::<u32>()) {
            parents.insert(child, parent);
        }
    }
    walk_ancestors(pid, &parents)
}

#[cfg(windows)]
fn windows_ancestors(pid: u32) -> Vec<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let mut parents = std::collections::HashMap::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return vec![pid];
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    walk_ancestors(pid, &parents)
}

#[cfg(unix)]
fn unix_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    std::io::Error::last_os_error()
        .raw_os_error()
        .is_some_and(|code| code == libc::EPERM)
}

#[cfg(windows)]
fn windows_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE as u32
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
