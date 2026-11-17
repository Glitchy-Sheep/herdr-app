extern crate self as gpui;
#[cfg(not(windows))]
pub struct Window(pub Option<std::num::NonZeroU32>);
#[cfg(windows)]
pub struct Window(pub std::num::NonZeroIsize);
impl raw_window_handle::HasWindowHandle for Window {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        #[cfg(windows)]
        {
            let raw = raw_window_handle::RawWindowHandle::Win32(
                raw_window_handle::Win32WindowHandle::new(self.0),
            );
            Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
        }
        #[cfg(not(windows))]
        if let Some(id) = self.0 {
            let raw = raw_window_handle::RawWindowHandle::Xcb(
                raw_window_handle::XcbWindowHandle::new(id),
            );
            Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
        } else {
            Err(raw_window_handle::HandleError::Unavailable)
        }
    }
}
pub mod mods {
    pub enum Request {
        Run {
            name: String,
            args: serde_json::Value,
        },
        OpenOverlay {
            id: String,
            spec: serde_json::Value,
        },
        QuickTerminalAutoHide,
        QuickTerminalShortcutError(Option<String>),
    }
    pub static TOGGLES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    pub fn request(request: Request) {
        match request {
            Request::Run { name, .. } if name == "quick_terminal.toggle" => {
                TOGGLES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Request::QuickTerminalShortcutError(error) => {
                *SHORTCUT_ERROR.lock().unwrap() = error;
            }
            Request::QuickTerminalAutoHide => {
                HIDES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Request::OpenOverlay { spec, .. } => {
                ERRORS.lock().unwrap().push(spec.to_string());
            }
            _ => panic!("unexpected native request"),
        }
    }
    pub static SHORTCUT_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    pub static ERRORS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    pub static HIDES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    pub fn new_overlay_id() -> String {
        "test-error".into()
    }
}
pub mod quick_terminal {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    pub static RECORDING: AtomicBool = AtomicBool::new(false);
    pub static RECORDINGS: AtomicUsize = AtomicUsize::new(0);
    pub static RECORDED: Mutex<Option<String>> = Mutex::new(None);
    pub fn record_shortcut(shortcut: &str) -> bool {
        if !RECORDING.load(Ordering::SeqCst) {
            return false;
        }
        *RECORDED.lock().unwrap() = Some(shortcut.to_owned());
        RECORDINGS.fetch_add(1, Ordering::SeqCst);
        true
    }
}
pub struct Config {
    pub shortcut: String,
    pub width: f32,
    pub height: f32,
    pub auto_hide: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedSize {
    pub width: f32,
    pub height: f32,
}
#[cfg(windows)]
#[path = "../../../src/quick_terminal/windows.rs"]
pub mod native;
#[cfg(not(windows))]
#[path = "../../../src/quick_terminal/linux.rs"]
pub mod native;
#[path = "../../../src/quick_terminal/release.rs"]
pub mod release;
