//! Physical modifier taps in the focused Windows shortcut field.

use super::windows::double_tap::{DoubleTap, ModifierKey, TapPhase};
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use std::{cell::RefCell, ptr::null_mut, sync::Mutex};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::SystemInformation::{GetTickCount, GetTickCount64};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardState, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RCONTROL, VK_RMENU,
    VK_RSHIFT, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, HHOOK, MSG, PM_REMOVE, SetWindowsHookExW,
    UnhookWindowsHookEx, WH_GETMESSAGE, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
    WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
};

thread_local! {
    static FIELD: RefCell<Option<Field>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum RecordingPeriod {
    #[default]
    Never,
    ActiveSince(u64),
    FinishedAt(u64),
}

static PERIOD: Mutex<RecordingPeriod> = Mutex::new(RecordingPeriod::Never);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ShortcutTarget {
    Terminal,
    Recorder,
    Ignore,
}

impl RecordingPeriod {
    pub(super) fn current() -> Self {
        *PERIOD.lock().unwrap()
    }

    pub(super) fn target(self, posted: u64) -> ShortcutTarget {
        match self {
            Self::Never => ShortcutTarget::Terminal,
            Self::ActiveSince(start) if posted > start => ShortcutTarget::Recorder,
            Self::FinishedAt(end) if posted > end => ShortcutTarget::Terminal,
            // Events on the boundary share one Windows clock tick with the transition.
            _ => ShortcutTarget::Ignore,
        }
    }
}

pub(crate) fn shortcut_target(posted: u64) -> ShortcutTarget {
    RecordingPeriod::current().target(posted)
}

pub(super) fn message_time(time: u32) -> u64 {
    expand_message_time(time, unsafe { GetTickCount64() })
}

// MSG.time contains the low 32 bits of uptime when the message was posted.
fn expand_message_time(time: u32, now: u64) -> u64 {
    now.saturating_sub(u64::from((now as u32).wrapping_sub(time)))
}

struct Field {
    window: HWND,
    taps: DoubleTap,
    recorded: Option<ModifierKey>,
}

pub struct Recorder(HHOOK);

impl Recorder {
    pub fn start(window: &Window) -> Result<Self, String> {
        let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window)
            .map_err(|e| e.to_string())?
            .as_raw()
        else {
            return Err("Shortcut recording needs a Windows window".into());
        };
        if FIELD.with_borrow(|field| field.is_some()) {
            return Err("A shortcut is already being recorded".into());
        }
        // Herdr's own message queue retains physical sides before GPUI merges them.
        let hook = unsafe {
            SetWindowsHookExW(
                WH_GETMESSAGE,
                Some(field_message),
                null_mut(),
                GetCurrentThreadId(),
            )
        };
        if hook.is_null() {
            return Err(format!(
                "Cannot record modifier taps: {}",
                std::io::Error::last_os_error()
            ));
        }
        FIELD.with_borrow_mut(|field| {
            *field = Some(Field {
                window: handle.hwnd.get() as HWND,
                taps: DoubleTap::default(),
                recorded: None,
            })
        });
        *PERIOD.lock().unwrap() = RecordingPeriod::ActiveSince(unsafe { GetTickCount64() });
        Ok(Self(hook))
    }

    pub fn status(&self, status: Value) -> Value {
        FIELD.with_borrow(|slot| match slot {
            Some(field) => field.status(status, unsafe { GetTickCount() }),
            None => status,
        })
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        FIELD.with_borrow_mut(|field| *field = None);
        unsafe { UnhookWindowsHookEx(self.0) };
        *PERIOD.lock().unwrap() = RecordingPeriod::FinishedAt(unsafe { GetTickCount64() });
    }
}

unsafe extern "system" fn field_message(code: i32, removed: WPARAM, data: LPARAM) -> LRESULT {
    if code >= 0 && removed == PM_REMOVE as usize {
        let message = unsafe { &*(data as *const MSG) };
        FIELD.with_borrow_mut(|slot| {
            if let Some(field) = slot
                && field.window == message.hwnd
                && field.recorded.is_none()
            {
                field.observe(
                    message,
                    || unsafe { GetForegroundWindow() },
                    modifier_is_alone,
                );
            }
        });
    }
    unsafe { CallNextHookEx(null_mut(), code, removed, data) }
}

impl Field {
    fn status(&self, status: Value, now: u32) -> Value {
        if status["state"] != "recording" {
            return status;
        }
        if let Some(key) = self.recorded {
            return json!({ "state": "recorded", "shortcut": key.shortcut(), "ready": false, "phase": "releasing" });
        }
        // A preview can expire while an on-time second tap is still queued.
        let mut preview = self.taps.clone();
        preview.refresh(now);
        let (key, phase) = match preview.phase() {
            TapPhase::Down(key) => (key, "modifier-down"),
            TapPhase::Waiting(key) => (key, "awaiting-second-tap"),
            TapPhase::Idle if status["preview"].is_null() => return status,
            TapPhase::Idle => {
                return json!({ "state": "recording", "phase": "idle", "ready": false });
            }
        };
        json!({ "state": "recording", "preview": key.shortcut().trim_start_matches("double-"), "phase": phase, "ready": false })
    }

    fn observe(
        &mut self,
        message: &MSG,
        foreground: impl FnOnce() -> HWND,
        alone: impl FnOnce(u16) -> bool,
    ) {
        let down = match message.message {
            WM_KEYDOWN | WM_SYSKEYDOWN => true,
            WM_KEYUP | WM_SYSKEYUP => false,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_MOUSEWHEEL
            | WM_MOUSEHWHEEL => {
                self.taps.interrupt();
                return;
            }
            _ => return,
        };
        if foreground() != self.window {
            self.taps.interrupt();
            return;
        }
        let vk = sided_key(message.wParam as u16, message.lParam);
        let Some(key) = ModifierKey::from_vk(vk) else {
            self.taps.interrupt();
            return;
        };
        if down && message.lParam & (1 << 30) != 0 {
            return;
        }
        if down && !alone(vk) {
            self.taps.interrupt();
        } else if self.taps.key(key, down, message.time) {
            self.recorded = Some(key);
        }
    }
}

fn sided_key(vk: u16, flags: LPARAM) -> u16 {
    let extended = flags & (1 << 24) != 0;
    match vk {
        VK_SHIFT if (flags >> 16) & 0xff == 0x36 => VK_RSHIFT,
        VK_SHIFT => VK_LSHIFT,
        VK_CONTROL if extended => VK_RCONTROL,
        VK_CONTROL => VK_LCONTROL,
        VK_MENU if extended => VK_RMENU,
        VK_MENU => VK_LMENU,
        _ => vk,
    }
}

fn modifier_is_alone(key: u16) -> bool {
    let mut state = [0u8; 256];
    let read = unsafe { GetKeyboardState(state.as_mut_ptr()) };
    read != 0
        && state.iter().enumerate().all(|(vk, byte)| {
            let vk = vk as u16;
            byte & 0x80 == 0 || vk == key || matches!(vk, VK_CONTROL | VK_MENU | VK_SHIFT)
        })
}

#[cfg(test)]
mod tests {
    use super::super::release::{Drain, released_status};
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_TAB;

    #[test]
    fn queued_hotkeys_belong_to_the_recording_in_which_they_were_posted() {
        let wrap = u64::from(u32::MAX) + 1;
        let from_recorder = expand_message_time(u32::MAX - 1, wrap + 50);
        assert_eq!(from_recorder, wrap - 2);
        let active = RecordingPeriod::ActiveSince(wrap - 10);
        assert_eq!(active.target(from_recorder), ShortcutTarget::Recorder);
        let closed = RecordingPeriod::FinishedAt(wrap);
        assert_eq!(closed.target(from_recorder), ShortcutTarget::Ignore);
        assert_eq!(closed.target(wrap), ShortcutTarget::Ignore);
        assert_eq!(closed.target(wrap + 1), ShortcutTarget::Terminal);
        let reopened = RecordingPeriod::ActiveSince(wrap + 6);
        assert_eq!(reopened.target(wrap + 1), ShortcutTarget::Ignore);
        assert_eq!(reopened.target(wrap + 6), ShortcutTarget::Ignore);
        assert_eq!(reopened.target(wrap + 7), ShortcutTarget::Recorder);
    }

    #[test]
    fn queued_key_messages_preserve_the_modifier_side() {
        for (vk, bits, expected) in [
            (VK_MENU, 0, VK_LMENU),
            (VK_MENU, 1 << 24, VK_RMENU),
            (VK_SHIFT, 0x2a << 16, VK_LSHIFT),
            (VK_SHIFT, 0x36 << 16, VK_RSHIFT),
            (VK_CONTROL, 0, VK_LCONTROL),
            (VK_CONTROL, 1 << 24, VK_RCONTROL),
        ] {
            assert_eq!(sided_key(vk, bits), expected);
        }
    }

    #[test]
    fn recording_alt_after_an_interrupted_chord_commits_only_on_second_release() {
        let mut field = Field {
            window: null_mut(),
            taps: DoubleTap::default(),
            recorded: None,
        };
        let idle = json!({ "state": "recording", "phase": "idle", "ready": false });
        let key = |field: &mut Field, message, vk: u16, time| {
            let msg = MSG {
                message,
                wParam: vk.into(),
                time,
                ..MSG::default()
            };
            field.observe(&msg, null_mut, |_| true);
        };
        key(&mut field, WM_SYSKEYDOWN, VK_MENU, 10);
        key(&mut field, WM_SYSKEYDOWN, VK_TAB, 20);
        key(&mut field, WM_SYSKEYUP, VK_TAB, 30);
        key(&mut field, WM_SYSKEYUP, VK_MENU, 40);
        assert_eq!(field.status(idle.clone(), 41), idle);

        let mut drain = Drain::default();
        key(&mut field, WM_SYSKEYDOWN, VK_MENU, 100);
        assert_eq!(field.status(idle.clone(), 105)["phase"], "modifier-down");
        key(&mut field, WM_SYSKEYUP, VK_MENU, 120);
        assert!(!drain.press("alt"));
        let waiting = field.status(idle.clone(), 130);
        assert_eq!(waiting["preview"], "left-alt");
        assert_eq!(waiting["phase"], "awaiting-second-tap");
        assert_eq!(field.status(waiting, 521), idle);

        // The UI was idle. Both messages were queued within the tap interval.
        key(&mut field, WM_SYSKEYDOWN, VK_MENU, 200);
        assert_eq!(field.status(idle.clone(), 201)["state"], "recording");
        key(&mut field, WM_SYSKEYUP, VK_MENU, 230);
        assert!(!drain.press("alt"));
        let status = field.status(idle, 600);
        assert_eq!(status["shortcut"], "double-left-alt");
        drain.native_pending = true;
        assert_eq!(
            released_status(status.clone(), &drain, false)["ready"],
            false
        );
        drain.native_pending = false;
        let complete = released_status(status, &drain, false);
        assert_eq!(complete["phase"], "complete");
        assert_eq!(complete["ready"], true);
        let chord = json!({ "state": "recorded", "shortcut": "ctrl-alt-f8" });
        assert_eq!(field.status(chord.clone(), 750), chord);
    }
}
