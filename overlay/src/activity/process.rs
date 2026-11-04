//! Process and window queries: which window a Claude Code session runs in,
//! and whether that window is the one the user is looking at.

use std::collections::HashMap;
use std::path::Path;

use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MENU};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetAncestor, GetClassNameW, GetForegroundWindow, GetWindow,
    GetWindowLongW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, SetForegroundWindow, ShowWindow, GA_ROOTOWNER, GWL_EXSTYLE, GW_OWNER, SW_RESTORE,
    WS_EX_TOOLWINDOW,
};

/// Processes that own windows but never host a Claude Code session.
const NOT_HOSTS: &[&str] = &[
    "explorer.exe",
    "claude-code-usage-monitor.exe",
    "svchost.exe",
    "services.exe",
    "wininit.exe",
    "winlogon.exe",
    "sihost.exe",
    "dwm.exe",
];

/// Window classes that are terminals; their process is not in the session's
/// parent chain, so they are only trusted as the foreground window.
const TERMINAL_CLASSES: &[&str] = &["ConsoleWindowClass", "CASCADIA_HOSTING_WINDOW_CLASS", "mintty"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Host {
    pub hwnd: i64,
    pub pid: u32,
    pub exe: String,
}

/// Work out the window the current hook's session runs in.
///
/// `prompt_submitted` is true for UserPromptSubmit: the user just typed in
/// that window, so the foreground window is the best evidence there is.
pub fn find_host(cwd: &str, prompt_submitted: bool) -> Host {
    let chain = parent_chain();
    let chain_pids: Vec<u32> = chain.iter().map(|(pid, _)| *pid).collect();
    let chain_exes: Vec<String> = chain
        .iter()
        .map(|(_, exe)| exe.to_ascii_lowercase())
        .filter(|exe| !NOT_HOSTS.contains(&exe.as_str()))
        .collect();

    if prompt_submitted {
        let foreground = unsafe { GetForegroundWindow() };
        if !foreground.is_invalid() {
            let root = root_window(foreground);
            let pid = window_pid(root);
            let exe = exe_name(pid).unwrap_or_default();
            let lower = exe.to_ascii_lowercase();
            let class = class_name(root);
            let related = chain_pids.contains(&pid)
                || chain_exes.contains(&lower)
                || TERMINAL_CLASSES.contains(&class.as_str());
            if related && !NOT_HOSTS.contains(&lower.as_str()) {
                return Host { hwnd: root.0 as i64, pid, exe };
            }
        }
    }

    let folder = Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_lowercase();
    for (pid, exe) in &chain {
        if NOT_HOSTS.contains(&exe.to_ascii_lowercase().as_str()) {
            continue;
        }
        let windows = app_windows(*pid);
        if windows.is_empty() {
            continue;
        }
        let titled = (!folder.is_empty())
            .then(|| windows.iter().find(|(_, title, _)| title.to_lowercase().contains(&folder)))
            .flatten();
        let chosen = titled.or_else(|| windows.iter().max_by_key(|(_, _, area)| *area));
        if let Some((hwnd, _, _)) = chosen {
            return Host { hwnd: hwnd.0 as i64, pid: *pid, exe: exe.clone() };
        }
    }
    Host::default()
}

/// The Claude Code process running this hook: the nearest ancestor that is
/// a known Claude Code runtime. None when it cannot be told apart, in which
/// case liveness falls back to the inactivity timeout.
pub fn find_agent() -> Option<(u32, String)> {
    const AGENTS: &[&str] = &["claude.exe", "node.exe", "bun.exe"];
    parent_chain()
        .into_iter()
        .take(4)
        .find(|(_, exe)| AGENTS.contains(&exe.to_ascii_lowercase().as_str()))
}

/// True while `pid` is still the same program that wrote the record.
pub fn process_alive(pid: u32, exe: &str) -> bool {
    match exe_name(pid) {
        Some(name) => exe.is_empty() || name.eq_ignore_ascii_case(exe),
        None => false,
    }
}

/// Is the session's window the active one?
pub fn host_is_foreground(host_hwnd: i64, host_exe: &str, foreground: &Foreground) -> bool {
    if foreground.hwnd == 0 {
        return false;
    }
    if host_hwnd != 0 {
        let hwnd = HWND(host_hwnd as *mut _);
        if unsafe { IsWindow(Some(hwnd)).as_bool() } {
            return foreground.root == host_hwnd && !unsafe { IsIconic(hwnd).as_bool() };
        }
    }
    !host_exe.is_empty() && foreground.exe.eq_ignore_ascii_case(host_exe)
}

/// Snapshot of the active window, taken once per poll.
#[derive(Clone, Debug, Default)]
pub struct Foreground {
    pub hwnd: i64,
    pub root: i64,
    pub exe: String,
}

pub fn foreground() -> Foreground {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return Foreground::default();
    }
    let root = root_window(hwnd);
    Foreground {
        hwnd: hwnd.0 as i64,
        root: root.0 as i64,
        exe: exe_name(window_pid(root)).unwrap_or_default(),
    }
}

/// Bring a session's window forward. Must run while handling a click so
/// Windows lets this process change the foreground window.
pub fn activate(host_hwnd: i64) -> bool {
    if host_hwnd == 0 {
        return false;
    }
    let hwnd = HWND(host_hwnd as *mut _);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd).as_bool() {
            return true;
        }
        // Foreground lock: a synthetic Alt press marks this thread as the
        // last input source, which lifts the restriction for one call.
        keybd_event(VK_MENU.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        let _ = BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd).as_bool()
    }
}

fn root_window(hwnd: HWND) -> HWND {
    let root = unsafe { GetAncestor(hwnd, GA_ROOTOWNER) };
    if root.is_invalid() {
        hwnd
    } else {
        root
    }
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}

/// File name of a process's executable, e.g. `Code.exe`.
pub fn exe_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        result.ok()?;
        let path = String::from_utf16_lossy(&buffer[..len as usize]);
        Path::new(&path).file_name().and_then(|name| name.to_str()).map(str::to_owned)
    }
}

/// Visible, unowned application windows of a process: (hwnd, title, area).
fn app_windows(pid: u32) -> Vec<(HWND, String, i64)> {
    struct Search {
        pid: u32,
        found: Vec<(HWND, String, i64)>,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        if window_pid(hwnd) != search.pid || !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        if GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
            return true.into();
        }
        if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return true.into();
        }
        let mut title = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut title);
        if len <= 0 {
            return true.into();
        }
        let mut rect = Default::default();
        let area = if GetWindowRect(hwnd, &mut rect).is_ok() {
            (rect.right - rect.left) as i64 * (rect.bottom - rect.top) as i64
        } else {
            0
        };
        search
            .found
            .push((hwnd, String::from_utf16_lossy(&title[..len as usize]), area));
        true.into()
    }

    let mut search = Search { pid, found: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

/// This process's ancestors, nearest first: (pid, exe file name).
fn parent_chain() -> Vec<(u32, String)> {
    let table = process_table();
    let mut chain = Vec::new();
    let mut pid = std::process::id();
    for _ in 0..24 {
        let Some((parent, _)) = table.get(&pid) else { break };
        let parent = *parent;
        if parent == 0 || parent == pid || chain.iter().any(|(seen, _)| *seen == parent) {
            break;
        }
        let Some((_, exe)) = table.get(&parent) else { break };
        chain.push((parent, exe.clone()));
        pid = parent;
    }
    chain
}

// Toolhelp is declared here rather than through a new `windows` crate
// feature, so Cargo.toml stays identical to upstream.
#[repr(C)]
#[allow(non_snake_case)]
struct ProcessEntry32W {
    dwSize: u32,
    cntUsage: u32,
    th32ProcessID: u32,
    th32DefaultHeapID: usize,
    th32ModuleID: u32,
    cntThreads: u32,
    th32ParentProcessID: u32,
    pcPriClassBase: i32,
    dwFlags: u32,
    szExeFile: [u16; 260],
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
    fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
    fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
}

/// pid -> (parent pid, exe file name)
fn process_table() -> HashMap<u32, (u32, String)> {
    const TH32CS_SNAPPROCESS: u32 = 0x2;
    let mut table = HashMap::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == -1 || snapshot == 0 {
            return table;
        }
        let mut entry: ProcessEntry32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<ProcessEntry32W>() as u32;
        let mut ok = Process32FirstW(snapshot, &mut entry) != 0;
        while ok {
            let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            table.insert(
                entry.th32ProcessID,
                (
                    entry.th32ParentProcessID,
                    String::from_utf16_lossy(&entry.szExeFile[..len]),
                ),
            );
            ok = Process32NextW(snapshot, &mut entry) != 0;
        }
        let _ = CloseHandle(windows::Win32::Foundation::HANDLE(snapshot as *mut _));
    }
    table
}
