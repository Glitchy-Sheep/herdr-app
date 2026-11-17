//! A managed topmost window and a thread-owned Win32 global hotkey.

#[path = "chord.rs"]
mod chord;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use std::ptr::null_mut;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey, VkKeyScanW,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, GWL_STYLE, GetCursorPos, GetForegroundWindow, GetMessageW,
    GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, HWND_TOPMOST, IsIconic, IsWindow,
    IsWindowVisible, IsZoomed, MINMAXINFO, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    SW_HIDE, SW_RESTORE, SW_SHOW, SWP_FRAMECHANGED, SWP_SHOWWINDOW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, WINEVENT_OUTOFCONTEXT, WM_APP, WM_ENTERSIZEMOVE,
    WM_GETMINMAXINFO, WM_HOTKEY, WM_NCDESTROY, WM_QUIT, WM_SIZING, WMSZ_BOTTOMLEFT,
    WMSZ_BOTTOMRIGHT, WMSZ_LEFT, WMSZ_RIGHT, WMSZ_TOPLEFT, WMSZ_TOPRIGHT, WS_MAXIMIZEBOX,
};

const HOTKEY_ID: i32 = 0x4844;
const SHOW: u32 = WM_APP + 1;
const HIDE: u32 = WM_APP + 2;

pub fn validate_shortcut(text: &str) -> Result<(), String> {
    native_chord(&chord::parse(text)?).map(|_| ())
}

#[derive(Default)]
pub struct KeyNames;

impl KeyNames {
    pub fn new() -> Result<Self, String> {
        Ok(Self)
    }

    pub fn normalize(&self, key: &str) -> String {
        let mut characters = key.encode_utf16();
        if let Some(character) = characters.next().filter(|_| characters.next().is_none()) {
            let mapping = unsafe { VkKeyScanW(character) };
            if mapping != -1 {
                let key = mapping & 0xff;
                if (0x30..=0x39).contains(&key) || (0x41..=0x5a).contains(&key) {
                    return (key as u8 as char).to_ascii_lowercase().to_string();
                }
                if key == 0xc0 {
                    return "backquote".into();
                }
                return format!("keycode-{key}");
            }
        }
        key.to_lowercase()
    }

    pub fn keyboard_released(&self) -> Result<bool, String> {
        Ok((0x08..=0xfe).all(|key| unsafe { GetAsyncKeyState(key) >= 0 }))
    }
}

thread_local! {
    static FOCUS: RefCell<Option<FocusState>> = const { RefCell::new(None) };
}

struct FocusState {
    window: usize,
    process: u32,
    was_active: bool,
    auto_hide: Arc<AtomicBool>,
}

unsafe extern "system" fn foreground_changed(
    _: HWINEVENTHOOK,
    _: u32,
    _: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    let hwnd = unsafe { GetForegroundWindow() };
    FOCUS.with(|focus| {
        let mut focus = focus.borrow_mut();
        let Some(focus) = focus.as_mut() else { return };
        let mut foreground_process = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut foreground_process);
        }
        if hwnd as usize == focus.window {
            focus.was_active = true;
        } else if foreground_process != focus.process && focus.was_active {
            focus.was_active = false;
            if focus.auto_hide.load(Ordering::SeqCst) {
                crate::mods::request(crate::mods::Request::QuickTerminalAutoHide);
            }
        }
    });
}

fn request(name: &str) {
    crate::mods::request(crate::mods::Request::Run {
        name: name.into(),
        args: serde_json::Value::Null,
    });
}

fn report_error(error: String) {
    tracing::warn!("quick terminal: {error}");
    crate::mods::request(crate::mods::Request::OpenOverlay {
        id: crate::mods::new_overlay_id(),
        spec: serde_json::json!({ "kind": "toast", "notification": { "title": "Quick terminal", "body": error, "level": "error" } }),
    });
}

struct Hotkey {
    thread_id: u32,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Default)]
struct ResizeState {
    saved: Option<super::SavedSize>,
    observed: Option<(i32, i32)>,
    placing: bool,
    anchor: Option<RECT>,
    minimum: (i32, i32),
}

struct ResizeHook {
    hwnd: HWND,
    // The subclass borrows a stationary Arc handle and retains a clone across reentrant messages.
    #[allow(clippy::redundant_allocation)]
    state: Box<Arc<Mutex<ResizeState>>>,
}

impl ResizeHook {
    fn new(hwnd: HWND, state: Arc<Mutex<ResizeState>>) -> Result<Self, String> {
        let state = Box::new(state);
        let id = (&*state as *const Arc<Mutex<ResizeState>>) as usize;
        if unsafe { SetWindowSubclass(hwnd, Some(resize_window), id, id) } == 0 {
            return Err(format!(
                "cannot observe quick terminal resizing: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self { hwnd, state })
    }
}

impl Drop for ResizeHook {
    fn drop(&mut self) {
        let id = (&*self.state as *const Arc<Mutex<ResizeState>>) as usize;
        unsafe {
            RemoveWindowSubclass(self.hwnd, Some(resize_window), id);
        }
    }
}

unsafe extern "system" fn resize_window(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        unsafe {
            RemoveWindowSubclass(hwnd, Some(resize_window), id);
        }
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }
    // The callback retains only scalar resize state; it never calls back into GPUI.
    let state = unsafe { (&*(data as *const Arc<Mutex<ResizeState>>)).clone() };
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    if message == WM_ENTERSIZEMOVE {
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            state.lock().unwrap().anchor = Some(info.rcWork);
        }
    }
    if lparam == 0 {
        return result;
    }
    if message == WM_GETMINMAXINFO {
        let limits = unsafe { &*(lparam as *const MINMAXINFO) };
        state.lock().unwrap().minimum = (limits.ptMinTrackSize.x, limits.ptMinTrackSize.y);
    } else if message == WM_SIZING {
        let state = state.lock().unwrap();
        if let Some(area) = state.anchor {
            let rect = unsafe { &mut *(lparam as *mut RECT) };
            *rect = centered_resize(area, *rect, wparam as u32, state.minimum);
            return 1;
        }
    }
    result
}

fn centered_resize(area: RECT, proposed: RECT, edge: u32, minimum: (i32, i32)) -> RECT {
    let center_twice = area.left + area.right;
    let width = match edge {
        WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT => center_twice - 2 * proposed.left,
        WMSZ_RIGHT | WMSZ_TOPRIGHT | WMSZ_BOTTOMRIGHT => 2 * proposed.right - center_twice,
        _ => proposed.right - proposed.left,
    };
    let available_width = (area.right - area.left).max(1);
    let available_height = (area.bottom - area.top).max(1);
    let width = width.clamp(minimum.0.clamp(1, available_width), available_width);
    let height =
        (proposed.bottom - area.top).clamp(minimum.1.clamp(1, available_height), available_height);
    let left = area.left + (available_width - width) / 2;
    RECT {
        left,
        top: area.top,
        right: left + width,
        bottom: area.top + height,
    }
}

impl Hotkey {
    fn new(
        hwnd: HWND,
        config: &super::Config,
        pending: Arc<AtomicUsize>,
        resize: Arc<Mutex<ResizeState>>,
        auto_hide: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let chord = chord::parse(&config.shortcut)?;
        let (modifiers, key) = native_chord(&chord)?;
        let window = hwnd as usize;
        let shortcut = config.shortcut.clone();
        let (width, height) = (config.width, config.height);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("herdr-quick-hotkey".into())
            .spawn(move || unsafe {
                let mut message: MSG = std::mem::zeroed();
                // A queue must exist before another thread can request shutdown.
                PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE);
                if RegisterHotKey(null_mut(), HOTKEY_ID, modifiers | MOD_NOREPEAT, key) == 0 {
                    let _ = tx.send(Err(format!(
                        "cannot register quick terminal shortcut: {}",
                        std::io::Error::last_os_error()
                    )));
                    return;
                }
                let hwnd = window as HWND;
                FOCUS.with(|focus| {
                    *focus.borrow_mut() = Some(FocusState {
                        window,
                        process: GetCurrentProcessId(),
                        was_active: GetForegroundWindow() == hwnd,
                        auto_hide,
                    })
                });
                let hook = SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    null_mut(),
                    Some(foreground_changed),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                );
                if hook.is_null() {
                    let _ = tx.send(Err(format!(
                        "cannot observe quick terminal focus: {}",
                        std::io::Error::last_os_error()
                    )));
                    UnregisterHotKey(null_mut(), HOTKEY_ID);
                    return;
                }
                let mut native = WindowOps {
                    hwnd,
                    previous: null_mut(),
                    width,
                    height,
                    resize,
                };
                if tx.send(Ok(GetCurrentThreadId())).is_ok() {
                    loop {
                        let result = GetMessageW(&mut message, null_mut(), 0, 0);
                        if result <= 0 {
                            if result < 0 {
                                pending.fetch_and(!3, Ordering::SeqCst);
                                let message = format!(
                                    "Quick terminal shortcut listener failed: {}",
                                    std::io::Error::last_os_error()
                                );
                                crate::mods::request(
                                    crate::mods::Request::QuickTerminalShortcutError(Some(
                                        message.clone(),
                                    )),
                                );
                                report_error(message);
                            }
                            break;
                        }
                        if message.message == WM_HOTKEY && message.wParam == HOTKEY_ID as usize {
                            if !crate::quick_terminal::record_shortcut(&shortcut) {
                                request("quick_terminal.toggle");
                            }
                        } else if matches!(message.message, SHOW | HIDE) {
                            if pending.load(Ordering::SeqCst) != message.wParam {
                                continue;
                            }
                            let result = if message.message == SHOW {
                                native.show()
                            } else {
                                native.hide()
                            };
                            let generation = message.wParam;
                            let _ = pending.compare_exchange(
                                generation,
                                generation & !3,
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                            );
                            if let Err(error) = result {
                                report_error(error);
                            }
                        }
                    }
                }
                if !hook.is_null() {
                    UnhookWinEvent(hook);
                }
                UnregisterHotKey(null_mut(), HOTKEY_ID);
            })
            .map_err(|e| format!("cannot start quick terminal shortcut listener: {e}"))?;
        match rx.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                thread: Some(thread),
            }),
            result => {
                let _ = thread.join();
                Err(result
                    .ok()
                    .and_then(Result::err)
                    .unwrap_or_else(|| "quick terminal shortcut listener stopped".into()))
            }
        }
    }
}

impl Drop for Hotkey {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        // Window operations may be waiting on the UI thread during shutdown.
        drop(self.thread.take());
    }
}

pub struct Native {
    hwnd: HWND,
    pending: Arc<AtomicUsize>,
    hotkey: Hotkey,
    resize: Arc<Mutex<ResizeState>>,
    auto_hide: Arc<AtomicBool>,
    _resize_hook: ResizeHook,
}

struct WindowOps {
    hwnd: HWND,
    previous: HWND,
    width: f32,
    height: f32,
    resize: Arc<Mutex<ResizeState>>,
}

impl Native {
    pub fn keyboard_released(&self) -> Result<bool, String> {
        // Keyboard virtual keys start at Backspace; mouse buttons are excluded.
        KeyNames.keyboard_released()
    }

    pub fn new(window: &gpui::Window, config: &super::Config) -> Result<Self, String> {
        let hwnd = match HasWindowHandle::window_handle(window)
            .map_err(|e| e.to_string())?
            .as_raw()
        {
            RawWindowHandle::Win32(handle) => handle.hwnd.get() as HWND,
            _ => return Err("quick terminal requires a Win32 window".into()),
        };
        let pending = Arc::new(AtomicUsize::new(0));
        let resize = Arc::new(Mutex::new(ResizeState::default()));
        let resize_hook = ResizeHook::new(hwnd, resize.clone())?;
        let auto_hide = Arc::new(AtomicBool::new(config.auto_hide));
        let hotkey = Hotkey::new(
            hwnd,
            config,
            pending.clone(),
            resize.clone(),
            auto_hide.clone(),
        )?;
        Ok(Self {
            hwnd,
            pending,
            hotkey,
            resize,
            auto_hide,
            _resize_hook: resize_hook,
        })
    }

    pub fn set_auto_hide(&mut self, enabled: bool) {
        self.auto_hide.store(enabled, Ordering::SeqCst);
    }

    /// Windows preferences use logical pixels at 96 DPI, including the window frame.
    pub fn set_saved_size(&mut self, size: Option<super::SavedSize>) {
        self.resize.lock().unwrap().saved = size.filter(|size| {
            size.width.is_finite()
                && size.height.is_finite()
                && size.width > 0.0
                && size.height > 0.0
        });
    }

    pub fn resized(&mut self) -> Option<super::SavedSize> {
        if self.pending.load(Ordering::SeqCst) & 3 != 0
            || !self.is_visible()
            || unsafe { IsZoomed(self.hwnd) != 0 }
        {
            return None;
        }
        let current = window_size(self.hwnd)?;
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        if dpi == 0 {
            return None;
        }
        let mut resize = self.resize.lock().unwrap();
        if resize.placing || resize.observed.is_none() || resize.observed == Some(current) {
            return None;
        }
        resize.observed = Some(current);
        let size = super::SavedSize {
            width: current.0 as f32 * 96.0 / dpi as f32,
            height: current.1 as f32 * 96.0 / dpi as f32,
        };
        resize.saved = Some(size);
        Some(size)
    }

    pub fn show(&mut self) -> Result<(), String> {
        self.enqueue(SHOW, 1)
    }

    pub fn hide(&mut self) -> Result<(), String> {
        if !self.is_visible() {
            return Ok(());
        }
        self.enqueue(HIDE, 2)
    }

    pub fn is_visible(&self) -> bool {
        match self.pending.load(Ordering::SeqCst) & 3 {
            1 => true,
            2 => false,
            _ => unsafe { IsWindowVisible(self.hwnd) != 0 && IsIconic(self.hwnd) == 0 },
        }
    }

    fn enqueue(&self, message: u32, action: usize) -> Result<(), String> {
        let previous = self
            .pending
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                Some(((old & !3).wrapping_add(4)) | action)
            })
            .unwrap();
        let generation = ((previous & !3).wrapping_add(4)) | action;
        if unsafe { PostThreadMessageW(self.hotkey.thread_id, message, generation, 0) } == 0 {
            let _ = self.pending.compare_exchange(
                generation,
                previous,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            return Err(format!(
                "cannot queue quick terminal window action: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

impl WindowOps {
    fn show(&mut self) -> Result<(), String> {
        self.resize.lock().unwrap().placing = true;
        let result = self.place_and_show();
        let observed = window_size(self.hwnd);
        let mut resize = self.resize.lock().unwrap();
        resize.observed = observed;
        resize.placing = false;
        result
    }

    fn place_and_show(&mut self) -> Result<(), String> {
        unsafe {
            if IsWindow(self.hwnd) == 0 {
                return Err("quick terminal window is no longer available".into());
            }
            let style = GetWindowLongPtrW(self.hwnd, GWL_STYLE);
            if style & WS_MAXIMIZEBOX as isize != 0
                && SetWindowLongPtrW(self.hwnd, GWL_STYLE, style & !(WS_MAXIMIZEBOX as isize)) == 0
            {
                return Err(format!(
                    "cannot disable quick terminal maximization: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let foreground = GetForegroundWindow();
            if !foreground.is_null() && foreground != self.hwnd {
                self.previous = foreground;
            }
            let mut point: POINT = std::mem::zeroed();
            if GetCursorPos(&mut point) == 0 {
                return Err(format!(
                    "cannot locate quick terminal display: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return Err(format!(
                    "cannot read quick terminal display: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let area = info.rcWork;
            self.resize.lock().unwrap().anchor = Some(area);
            let available_width = area.right - area.left;
            let available_height = area.bottom - area.top;
            let mut dpi_x = 96;
            let mut dpi_y = 96;
            if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) < 0 {
                dpi_x = GetDpiForWindow(self.hwnd).max(96);
            }
            let saved = self.resize.lock().unwrap().saved;
            let (width, height) = if let Some(size) = saved {
                (
                    (size.width * dpi_x as f32 / 96.0).round() as i32,
                    (size.height * dpi_x as f32 / 96.0).round() as i32,
                )
            } else {
                (
                    (available_width as f32 * self.width).round() as i32,
                    (available_height as f32 * self.height).round() as i32,
                )
            };
            let width = width.clamp(1, available_width.max(1));
            let height = height.clamp(1, available_height.max(1));
            ShowWindow(
                self.hwnd,
                if IsIconic(self.hwnd) != 0 || IsZoomed(self.hwnd) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            if SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                area.left + (available_width - width) / 2,
                area.top,
                width,
                height,
                SWP_SHOWWINDOW | SWP_FRAMECHANGED,
            ) == 0
            {
                return Err(format!(
                    "cannot position quick terminal: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if SetForegroundWindow(self.hwnd) == 0 && GetForegroundWindow() != self.hwnd {
                return Err("Windows declined to focus the quick terminal; use its registered global shortcut".into());
            }
        }
        Ok(())
    }

    fn hide(&mut self) -> Result<(), String> {
        unsafe {
            if IsWindow(self.hwnd) == 0 {
                return Err("quick terminal window is no longer available".into());
            }
            let restore = GetForegroundWindow() == self.hwnd;
            ShowWindow(self.hwnd, SW_HIDE);
            if restore
                && !self.previous.is_null()
                && IsWindow(self.previous) != 0
                && IsIconic(self.previous) == 0
            {
                SetForegroundWindow(self.previous);
            }
        }
        Ok(())
    }
}

fn window_size(hwnd: HWND) -> Option<(i32, i32)> {
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return None;
    }
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    (width > 0 && height > 0).then_some((width, height))
}

fn native_chord(chord: &chord::Chord) -> Result<(u32, u32), String> {
    use chord::Key;
    let modifiers = if chord.ctrl { MOD_CONTROL } else { 0 }
        | if chord.alt { MOD_ALT } else { 0 }
        | if chord.shift { MOD_SHIFT } else { 0 }
        | if chord.super_key { MOD_WIN } else { 0 };
    let key = match chord.key {
        Key::Character(c) => c.to_ascii_uppercase() as u32,
        Key::Function(12) => {
            return Err("F12 is reserved by Windows and cannot be a global shortcut".into());
        }
        Key::Function(n) => 0x70 + n as u32 - 1,
        Key::Backquote => 0xc0,
        Key::Space => 0x20,
        Key::Escape => 0x1b,
        Key::Tab => 0x09,
        Key::Enter => 0x0d,
    };
    Ok((modifiers, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_match_win32_codes() {
        assert_eq!(
            native_chord(&chord::parse("ctrl-alt-backquote").unwrap()).unwrap(),
            (MOD_CONTROL | MOD_ALT, 0xc0)
        );
        assert_eq!(
            native_chord(&chord::parse("ctrl-a").unwrap()).unwrap().1,
            0x41
        );
        assert_eq!(
            native_chord(&chord::parse("alt-f24").unwrap()).unwrap().1,
            0x87
        );
        assert!(native_chord(&chord::parse("ctrl-f12").unwrap()).is_err());
        assert!(validate_shortcut("f8").is_ok());
        assert!(
            validate_shortcut("f12")
                .unwrap_err()
                .contains("reserved by Windows")
        );
        assert!(validate_shortcut("right-cmd-space").is_err());
    }

    #[test]
    fn mouse_resize_keeps_grabbed_edge_center_and_top() {
        let area = RECT {
            left: -1920,
            top: 40,
            right: 0,
            bottom: 1080,
        };
        let right = centered_resize(
            area,
            RECT {
                left: -1440,
                top: 40,
                right: -400,
                bottom: 600,
            },
            WMSZ_BOTTOMRIGHT,
            (200, 100),
        );
        assert_eq!(
            (right.left, right.top, right.right, right.bottom),
            (-1520, 40, -400, 600)
        );
        let left = centered_resize(
            area,
            RECT {
                left: -1600,
                top: 20,
                right: -400,
                bottom: 600,
            },
            WMSZ_TOPLEFT,
            (200, 100),
        );
        assert_eq!(
            (left.left, left.top, left.right, left.bottom),
            (-1600, 40, -320, 600)
        );
        let clamped = centered_resize(
            area,
            RECT {
                left: -2400,
                top: 40,
                right: 0,
                bottom: 1400,
            },
            WMSZ_BOTTOMLEFT,
            (200, 100),
        );
        assert_eq!(
            (clamped.left, clamped.top, clamped.right, clamped.bottom),
            (-1920, 40, 0, 1080)
        );
    }
}
