//! The animated badge that sits next to the usage widget.
//!
//! It runs on its own thread with its own window and message loop, finds the
//! widget by its window class and follows it around. Nothing in the upstream
//! window code knows it exists.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, MapWindowPoints, ReleaseDC,
    SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    DIB_RGB_COLORS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId, WaitForSingleObject, INFINITE};
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowExW, FindWindowW, GetMessageW, GetParent,
    GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible, KillTimer,
    LoadCursorW, PostThreadMessageW, RegisterClassW, SetCursor, SetParent, SetTimer, SetWindowLongW,
    SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow, GWL_EXSTYLE, GWL_STYLE, HTCLIENT,
    HWND_TOP, HWND_TOPMOST, IDC_HAND, MA_NOACTIVATE, MSG, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_APP, WM_DESTROY, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_NCHITTEST, WM_RBUTTONUP, WM_SETCURSOR, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::layout::{self, Rect};
use super::paint::{self, Look};
use super::process::{self, Foreground};
use super::settings::{self, ActivitySettings};
use super::store::{self, Observation, Phase, SessionRecord, Summary};

const CLASS: &str = "CCUMActivityBadge";
const WIDGET_CLASS: &str = "ClaudeCodeUsageSurface";
const TIMER_POLL: usize = 0xA11;
const TIMER_FRAME: usize = 0xA12;
const POLL_MS: u32 = 250;
const FRAME_MS: u32 = 33;
const FADE_SECONDS: f32 = 0.22;
const WM_APP_RECREATE: u32 = WM_APP + 0x3A1;

const WS_CHILD: u32 = 0x4000_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;

thread_local! {
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

pub fn run() {
    // One badge per desktop session. A relaunched monitor waits here until
    // the previous process is gone instead of giving up.
    let name = wide("Local\\ClaudeCodeUsageMonitor-ActivityBadge");
    let Ok(mutex) = (unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) }) else {
        return;
    };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            WaitForSingleObject(mutex, INFINITE);
        }
    }

    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let settings = ActivitySettings::load_or_create(&settings::settings_path());
    store::prune(&settings::sessions_dir(), store::now_ms(), 24 * 60 * 60_000);

    let started = store::now_ms();
    std::thread::spawn(move || {
        // Give the monitor a moment to settle; a duplicate launch exits
        // before this runs.
        std::thread::sleep(Duration::from_secs(4));
        let _ = super::ensure_hooks(false);
    });

    let hwnd = create_window();
    if hwnd.is_invalid() {
        super::log("activity: unable to create badge window");
        return;
    }
    OVERLAY.with(|cell| *cell.borrow_mut() = Some(Overlay::new(hwnd, settings, started)));
    super::log("activity: badge thread started");

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.hwnd.is_invalid() && msg.message == WM_APP_RECREATE {
                // Explorer restarted and took our child window with it.
                std::thread::sleep(Duration::from_millis(1500));
                let hwnd = create_window();
                OVERLAY.with(|cell| {
                    if let Ok(mut overlay) = cell.try_borrow_mut() {
                        if let Some(overlay) = overlay.as_mut() {
                            overlay.reset_window(hwnd);
                        }
                    }
                });
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn create_window() -> HWND {
    unsafe {
        let instance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let class = wide(CLASS);
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .unwrap_or_default();
        if !hwnd.is_invalid() {
            SetTimer(Some(hwnd), TIMER_POLL, POLL_MS, None);
        }
        hwnd
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            with_overlay(|overlay| match wparam.0 {
                TIMER_POLL => overlay.poll(),
                TIMER_FRAME => overlay.frame(),
                _ => {}
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_overlay(|overlay| overlay.click(true));
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            with_overlay(|overlay| overlay.click(false));
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_SETCURSOR => {
            if let Ok(cursor) = LoadCursorW(None, IDC_HAND) {
                SetCursor(Some(cursor));
            }
            LRESULT(1)
        }
        WM_DESTROY => {
            let current = OVERLAY.with(|cell| {
                cell.try_borrow()
                    .ok()
                    .and_then(|overlay| overlay.as_ref().map(|overlay| overlay.hwnd == hwnd))
                    .unwrap_or(true)
            });
            if current {
                let _ = PostThreadMessageW(GetCurrentThreadId(), WM_APP_RECREATE, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Run against the overlay state unless a nested message is already using it.
fn with_overlay(f: impl FnOnce(&mut Overlay)) {
    OVERLAY.with(|cell| {
        if let Ok(mut overlay) = cell.try_borrow_mut() {
            if let Some(overlay) = overlay.as_mut() {
                f(overlay);
            }
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placement {
    screen: Rect,
    /// Taskbar the badge is embedded in, or none for a floating popup.
    parent: isize,
}

#[derive(Default)]
struct TranscriptState {
    mtime_ms: u64,
    checked_mtime_ms: u64,
    interrupted: bool,
}

struct Overlay {
    hwnd: HWND,
    settings: ActivitySettings,
    settings_mtime: Option<SystemTime>,
    settings_checked: Instant,
    started_ms: u64,
    /// session -> updated_ms the user has already seen.
    acknowledged: HashMap<String, u64>,
    /// session -> turn whose working badge was dismissed.
    muted: HashMap<String, u64>,
    transcripts: HashMap<String, TranscriptState>,
    records: Vec<SessionRecord>,
    summary: Summary,
    /// Look being drawn; kept while fading out.
    look: Option<Look>,
    look_since: Instant,
    clock: Instant,
    opacity: f32,
    visible_target: bool,
    last_frame: Instant,
    frame_timer: bool,
    placement: Option<Placement>,
    shown: bool,
}

impl Overlay {
    fn new(hwnd: HWND, settings: ActivitySettings, started_ms: u64) -> Self {
        let now = Instant::now();
        Self {
            hwnd,
            settings_mtime: modified(&settings::settings_path()),
            settings,
            settings_checked: now,
            started_ms,
            acknowledged: HashMap::new(),
            muted: HashMap::new(),
            transcripts: HashMap::new(),
            records: Vec::new(),
            summary: Summary::default(),
            look: None,
            look_since: now,
            clock: now,
            opacity: 0.0,
            visible_target: false,
            last_frame: now,
            frame_timer: false,
            placement: None,
            shown: false,
        }
    }

    fn reset_window(&mut self, hwnd: HWND) {
        self.hwnd = hwnd;
        self.placement = None;
        self.shown = false;
        self.frame_timer = false;
        if self.visible_target || self.opacity > 0.0 {
            self.start_frames();
        }
    }

    fn poll(&mut self) {
        self.reload_settings();
        let now = store::now_ms();
        let foreground = process::foreground();
        self.records = if self.settings.enabled {
            store::list_records(&settings::sessions_dir())
                .into_iter()
                .map(|(_, record)| record)
                .collect()
        } else {
            Vec::new()
        };

        let timing = self.settings.timing();
        let mut looks = Vec::new();
        let records = std::mem::take(&mut self.records);
        for record in &records {
            let obs = self.observe(record, &foreground);
            if let Some(look) = store::session_look(record, obs, now, timing) {
                looks.push((record, look));
            }
        }
        self.summary = store::summarise(looks);
        self.records = records;
        let live: Vec<&str> = self.records.iter().map(|r| r.session_id.as_str()).collect();
        self.acknowledged.retain(|id, _| live.contains(&id.as_str()));
        self.muted.retain(|id, _| live.contains(&id.as_str()));

        match self.summary.look {
            Some(look) => {
                if self.look != Some(look) {
                    self.look = Some(look);
                    self.look_since = Instant::now();
                }
                self.visible_target = true;
            }
            None => self.visible_target = false,
        }

        if self.visible_target || self.opacity > 0.0 {
            self.place();
            self.start_frames();
        }
    }

    /// Gather focus, acknowledgement and transcript facts for one record.
    fn observe(&mut self, record: &SessionRecord, foreground: &Foreground) -> Observation {
        let focused = !self.settings.show_when_focused
            && process::host_is_foreground(record.host_hwnd, &record.host_exe, foreground);
        let needs_ack = matches!(record.phase, Phase::Attention | Phase::Failure | Phase::Done);
        // Looking at the session's window counts as having seen it; so does
        // anything that finished long before this badge started.
        let stale_at_start = record.updated_ms + 120_000 < self.started_ms;
        if needs_ack && (focused || stale_at_start) {
            self.acknowledged.insert(record.session_id.clone(), record.updated_ms);
        }
        let acknowledged = self.acknowledged.get(&record.session_id) == Some(&record.updated_ms);
        let muted = self.muted.get(&record.session_id) == Some(&record.turn_started_ms);

        // Session closed without SessionEnd (window closed, crash, killed):
        // its Claude Code process is gone, so forget the record.
        // Only trust it after a grace period, so a short-lived launcher can
        // never hide a session that just started.
        let gone = record.agent_pid != 0
            && store::now_ms().saturating_sub(record.updated_ms) > 60_000
            && !process::process_alive(record.agent_pid, &record.agent_exe);
        if gone {
            store::delete_record(&settings::sessions_dir(), &record.session_id);
        }
        let mut obs = Observation { focused, acknowledged, muted, gone, ..Default::default() };
        if matches!(record.phase, Phase::Working | Phase::Attention) && !record.transcript_path.is_empty() {
            let state = self.transcripts.entry(record.transcript_path.clone()).or_default();
            state.mtime_ms = modified(&PathBuf::from(&record.transcript_path))
                .map(store::system_time_ms)
                .unwrap_or(0);
            if state.mtime_ms != state.checked_mtime_ms {
                state.checked_mtime_ms = state.mtime_ms;
                state.interrupted = state.mtime_ms > record.updated_ms + 300
                    && read_tail(&record.transcript_path, 8 * 1024)
                        .is_some_and(|tail| store::tail_shows_interrupt(&tail));
            }
            obs.transcript_mtime_ms = state.mtime_ms;
            obs.interrupted = state.interrupted && state.mtime_ms > record.updated_ms;
        }
        obs
    }

    fn reload_settings(&mut self) {
        if self.settings_checked.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.settings_checked = Instant::now();
        let path = settings::settings_path();
        let mtime = modified(&path);
        if mtime != self.settings_mtime {
            self.settings_mtime = mtime;
            let previous = self.settings.clone();
            self.settings = ActivitySettings::load_or_create(&path);
            if self.settings.auto_install_hooks && !previous.auto_install_hooks {
                std::thread::spawn(|| {
                    let _ = super::ensure_hooks(false);
                });
            }
            self.placement = None;
        }
    }

    fn click(&mut self, left: bool) {
        let shown = self.summary.look;
        let primary = self.summary.primary.clone();
        let mut target = 0;
        let now = store::now_ms();
        let foreground = process::foreground();
        let timing = self.settings.timing();
        let records = self.records.clone();
        for record in &records {
            let obs = self.observe(record, &foreground);
            let look = store::session_look(record, obs, now, timing);
            if look.is_none() || look != shown {
                continue;
            }
            if primary.as_deref() == Some(record.session_id.as_str()) {
                target = record.host_hwnd;
            }
            if record.phase == Phase::Working || look == Some(Look::Working) {
                if !left {
                    self.muted.insert(record.session_id.clone(), record.turn_started_ms);
                }
            } else {
                self.acknowledged.insert(record.session_id.clone(), record.updated_ms);
            }
        }
        if left && !process::activate(target) {
            super::log("activity: no window to bring forward for this session");
        }
        self.poll();
    }

    fn start_frames(&mut self) {
        if self.frame_timer || self.hwnd.is_invalid() {
            return;
        }
        self.last_frame = Instant::now();
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_FRAME, FRAME_MS, None);
        }
        self.frame_timer = true;
    }

    fn stop_frames(&mut self) {
        if !self.frame_timer {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_FRAME);
        }
        self.frame_timer = false;
    }

    fn frame(&mut self) {
        let dt = self.last_frame.elapsed().as_secs_f32().min(0.25);
        self.last_frame = Instant::now();
        let step = dt / FADE_SECONDS;
        self.opacity = if self.visible_target {
            (self.opacity + step).min(1.0)
        } else {
            (self.opacity - step).max(0.0)
        };

        let Some(look) = self.look else {
            self.hide();
            return;
        };
        if self.opacity <= 0.0 && !self.visible_target {
            self.look = None;
            self.hide();
            return;
        }
        let Some(placement) = self.placement else {
            return;
        };
        let size = placement.screen.width().max(1) as u32;
        let t = self.look_since.elapsed().as_secs_f32();
        let clock = self.clock.elapsed().as_secs_f32();
        let pixels = paint::render(look, self.settings.style(), t, clock, size, self.summary.count, ease(self.opacity));
        present(self.hwnd, &pixels, size);
        if !self.shown {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            self.shown = true;
        }
    }

    fn hide(&mut self) {
        self.stop_frames();
        if self.shown {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.shown = false;
        }
    }

    /// Follow the usage widget; fall back to the notification area.
    fn place(&mut self) {
        let Some((anchor, bar)) = find_anchor() else {
            return;
        };
        let screen = layout::badge_rect(
            anchor,
            bar.map(|(_, rect)| rect),
            self.settings.side(),
            self.settings.size_percent,
        );
        let placement = Placement {
            screen,
            parent: bar.map(|(hwnd, _)| hwnd.0 as isize).unwrap_or(0),
        };
        let still_attached = unsafe { IsWindow(Some(self.hwnd)).as_bool() }
            && current_parent(self.hwnd) == placement.parent;
        if self.placement == Some(placement) && still_attached {
            return;
        }
        apply_placement(self.hwnd, placement);
        self.placement = Some(placement);
    }
}

fn ease(opacity: f32) -> f32 {
    let t = opacity.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Screen rect of the anchor and, when it lives in a taskbar, that taskbar.
fn find_anchor() -> Option<(Rect, Option<(HWND, Rect)>)> {
    let widget_class = wide(WIDGET_CLASS);
    let own_pid = std::process::id();
    let mut taskbars = Vec::new();
    unsafe {
        if let Ok(primary) = FindWindowW(PCWSTR(wide("Shell_TrayWnd").as_ptr()), PCWSTR::null()) {
            taskbars.push(primary);
        }
        let secondary = wide("Shell_SecondaryTrayWnd");
        let mut after: Option<HWND> = None;
        while let Ok(bar) = FindWindowExW(None, after, PCWSTR(secondary.as_ptr()), PCWSTR::null()) {
            if bar.is_invalid() || taskbars.contains(&bar) {
                break;
            }
            taskbars.push(bar);
            after = Some(bar);
        }

        for bar in &taskbars {
            let mut after: Option<HWND> = None;
            while let Ok(widget) = FindWindowExW(Some(*bar), after, PCWSTR(widget_class.as_ptr()), PCWSTR::null()) {
                if widget.is_invalid() {
                    break;
                }
                after = Some(widget);
                if window_pid(widget) == own_pid && IsWindowVisible(widget).as_bool() {
                    if let (Some(rect), Some(bar_rect)) = (window_rect(widget), window_rect(*bar)) {
                        return Some((rect, Some((*bar, bar_rect))));
                    }
                }
            }
        }

        // Floating themes keep the widget as a top-level popup.
        let mut after: Option<HWND> = None;
        while let Ok(widget) = FindWindowExW(None, after, PCWSTR(widget_class.as_ptr()), PCWSTR::null()) {
            if widget.is_invalid() {
                break;
            }
            after = Some(widget);
            if window_pid(widget) == own_pid && IsWindowVisible(widget).as_bool() {
                if let Some(rect) = window_rect(widget) {
                    return Some((rect, None));
                }
            }
        }

        // Widget hidden: sit beside the clock instead.
        let primary = *taskbars.first()?;
        let tray = FindWindowExW(Some(primary), None, PCWSTR(wide("TrayNotifyWnd").as_ptr()), PCWSTR::null()).ok()?;
        let tray_rect = window_rect(tray)?;
        let bar_rect = window_rect(primary)?;
        let horizontal = bar_rect.width() >= bar_rect.height();
        let anchor = if horizontal {
            Rect { top: bar_rect.top + 2, bottom: bar_rect.bottom - 2, ..tray_rect }
        } else {
            Rect { left: bar_rect.left + 2, right: bar_rect.right - 2, ..tray_rect }
        };
        Some((anchor, Some((primary, bar_rect))))
    }
}

fn current_parent(hwnd: HWND) -> isize {
    unsafe { GetParent(hwnd).map(|parent| parent.0 as isize).unwrap_or(0) }
}

fn apply_placement(hwnd: HWND, placement: Placement) {
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let r = placement.screen;
        if placement.parent != 0 {
            let parent = HWND(placement.parent as *mut _);
            if current_parent(hwnd) != placement.parent {
                // Same hosting the widget uses: a layered child of the taskbar
                // shares its visibility and z-order.
                let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, ((ex_style | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) & !WS_EX_TOPMOST.0) as i32);
                let _ = SetWindowLongW(hwnd, GWL_STYLE, ((style & !WS_POPUP.0) | WS_CHILD | WS_CLIPSIBLINGS) as i32);
                let _ = SetParent(hwnd, Some(parent));
                if current_parent(hwnd) == placement.parent {
                    // Windows 11 can leave a reparented layered window under
                    // the taskbar's composition visual; toggling rebinds it.
                    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                    let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_LAYERED.0) as i32);
                    let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, (ex | WS_EX_LAYERED.0) as i32);
                }
            }
            let mut point = [POINT { x: r.left, y: r.top }];
            MapWindowPoints(None, Some(parent), &mut point);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                point[0].x,
                point[0].y,
                r.width(),
                r.height(),
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        } else {
            if current_parent(hwnd) != 0 {
                let _ = SetParent(hwnd, None);
                let _ = SetWindowLongW(hwnd, GWL_STYLE, ((style & !(WS_CHILD | WS_CLIPSIBLINGS)) | WS_POPUP.0) as i32);
                let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_LAYERED.0) as i32);
                let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, (ex | WS_EX_LAYERED.0) as i32);
            }
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), r.left, r.top, r.width(), r.height(), SWP_NOACTIVATE);
        }
        let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// Push a premultiplied BGRA frame to the layered window.
fn present(hwnd: HWND, pixels: &[u32], size: u32) {
    let side = size as i32;
    if pixels.len() != (size * size) as usize {
        return;
    }
    unsafe {
        let screen_dc = GetDC(None);
        let memory_dc = CreateCompatibleDC(Some(screen_dc));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: side,
                biHeight: -side,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(memory_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(memory_dc);
            ReleaseDC(None, screen_dc);
            return;
        };
        if bits.is_null() {
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(memory_dc);
            ReleaseDC(None, screen_dc);
            return;
        }
        let old = SelectObject(memory_dc, bitmap.into());
        let target = std::slice::from_raw_parts_mut(bits as *mut u32, pixels.len());
        for (dst, src) in target.iter_mut().zip(pixels) {
            // Fully transparent pixels let clicks fall through to the
            // taskbar; keep the whole square clickable.
            *dst = if src >> 24 == 0 { 0x0100_0000 } else { *src };
        }
        let source = POINT { x: 0, y: 0 };
        let extent = SIZE { cx: side, cy: side };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            Some(screen_dc),
            None,
            Some(&extent),
            Some(memory_dc),
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        SelectObject(memory_dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory_dc);
        ReleaseDC(None, screen_dc);
    }
}

fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
    Some(Rect { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom })
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid
}

fn modified(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

fn read_tail(path: &str, bytes: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(bytes))).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
