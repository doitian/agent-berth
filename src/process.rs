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
    walk_ancestors(pid, &processes())
}

struct Process {
    parent: u32,
    name: String,
}

pub fn ancestor_named(name: &str) -> Option<u32> {
    let processes = processes();
    find_ancestor(std::process::id(), name, &processes)
}

fn find_ancestor(
    pid: u32,
    name: &str,
    processes: &std::collections::HashMap<u32, Process>,
) -> Option<u32> {
    walk_ancestors(pid, processes)
        .into_iter()
        .skip(1)
        .find(|pid| {
            processes.get(pid).is_some_and(|process| {
                std::path::Path::new(&process.name)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|filename| {
                        filename == name || filename.eq_ignore_ascii_case(&format!("{name}.exe"))
                    })
            })
        })
}

fn walk_ancestors(pid: u32, processes: &std::collections::HashMap<u32, Process>) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut current = pid;
    while chain.len() < 256 {
        let Some(process) = processes.get(&current) else {
            break;
        };
        let parent = process.parent;
        if parent == 0 || chain.contains(&parent) {
            break;
        }
        chain.push(parent);
        current = parent;
    }
    chain
}

#[cfg(unix)]
fn processes() -> std::collections::HashMap<u32, Process> {
    let output = std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output();
    let mut processes = std::collections::HashMap::new();
    let Ok(output) = output else {
        return processes;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if let Some((pid, process)) = parse_process(line) {
            processes.insert(pid, process);
        }
    }
    processes
}

#[cfg(any(unix, test))]
fn parse_process(line: &str) -> Option<(u32, Process)> {
    let (pid, rest) = line.trim_start().split_once(char::is_whitespace)?;
    let (parent, name) = rest.trim_start().split_once(char::is_whitespace)?;
    Some((
        pid.parse().ok()?,
        Process {
            parent: parent.parse().ok()?,
            name: name.trim().to_string(),
        },
    ))
}

#[cfg(windows)]
fn processes() -> std::collections::HashMap<u32, Process> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let mut processes = std::collections::HashMap::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return processes;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&ch| ch == 0)
                    .unwrap_or(entry.szExeFile.len());
                processes.insert(
                    entry.th32ProcessID,
                    Process {
                        parent: entry.th32ParentProcessID,
                        name: String::from_utf16_lossy(&entry.szExeFile[..len]),
                    },
                );
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    processes
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
