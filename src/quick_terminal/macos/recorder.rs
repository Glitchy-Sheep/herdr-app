//! Local capture precedes GPUI bindings and native menu key equivalents.

use super::shortcut::{Capture, CaptureResult};
use super::*;

pub struct Recorder {
    capture: Rc<RefCell<Capture>>,
    monitor: id,
    observer: id,
    window: id,
    active: bool,
}

impl Recorder {
    /// Local recording requires the application's global hotkey to be suspended.
    pub fn start(window: &Window, owner: u64) -> Result<Self, String> {
        if RECORDING.get() {
            return Err("A shortcut is already being recorded".into());
        }
        let view = match HasWindowHandle::window_handle(window)
            .map_err(|e| e.to_string())?
            .as_raw()
        {
            RawWindowHandle::AppKit(handle) => handle.ns_view.as_ptr() as id,
            _ => return Err("Shortcut recording needs an AppKit window".into()),
        };
        unsafe {
            let window: id = msg_send![view, window];
            if window.is_null() {
                return Err("Shortcut recording has no native window".into());
            }
            let _: id = msg_send![window, retain];
            let mut recorder = Self {
                capture: Rc::default(),
                monitor: nil,
                observer: nil,
                window,
                active: true,
            };
            let flags: u64 = msg_send![class!(NSEvent), modifierFlags];
            recorder.capture.borrow_mut().set_modifiers(flags);
            RECORDING.set(true);
            let capture = recorder.capture.clone();
            let block = ConcreteBlock::new(move |event: id| {
                let mut capture = capture.borrow_mut();
                let key_window: BOOL = msg_send![window, isKeyWindow];
                if key_window != YES {
                    if !capture.ready() {
                        capture.cancel();
                    }
                    crate::quick_terminal::publish_recording(owner, snapshot(&capture));
                    return event;
                }
                let kind: u64 = msg_send![event, type];
                let consumed = match kind {
                    KEY_DOWN => {
                        let key: u16 = msg_send![event, keyCode];
                        let flags: u64 = msg_send![event, modifierFlags];
                        let repeated: BOOL = msg_send![event, isARepeat];
                        capture.key_down(key, flags, repeated == YES)
                    }
                    KEY_UP => {
                        let key: u16 = msg_send![event, keyCode];
                        let flags: u64 = msg_send![event, modifierFlags];
                        capture.key_up(key, flags)
                    }
                    FLAGS_CHANGED => {
                        let key: u16 = msg_send![event, keyCode];
                        let flags: u64 = msg_send![event, modifierFlags];
                        let at: f64 = msg_send![event, timestamp];
                        capture.flags(key, flags, at)
                    }
                    _ => {
                        capture.reset_taps();
                        false
                    }
                };
                crate::quick_terminal::publish_recording(owner, snapshot(&capture));
                if consumed { nil } else { event }
            })
            .copy();
            let monitor: id = msg_send![class!(NSEvent), addLocalMonitorForEventsMatchingMask: INPUT_MASK handler: &*block];
            if monitor.is_null() {
                return Err("macOS could not start shortcut recording".into());
            }
            let _: id = msg_send![monitor, retain];
            recorder.monitor = monitor;

            let capture = recorder.capture.clone();
            let block = ConcreteBlock::new(move |_notification: id| {
                let mut capture = capture.borrow_mut();
                if !capture.ready() {
                    capture.cancel();
                }
                crate::quick_terminal::publish_recording(owner, snapshot(&capture));
            })
            .copy();
            let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
            let name = NSString::alloc(nil).init_str("NSWindowDidResignKeyNotification");
            let observer: id = msg_send![center, addObserverForName: name object: window queue: nil usingBlock: &*block];
            let _: () = msg_send![name, release];
            if observer.is_null() {
                return Err("macOS could not observe recording focus".into());
            }
            let _: id = msg_send![observer, retain];
            recorder.observer = observer;
            Ok(recorder)
        }
    }

    pub fn status(&self) -> serde_json::Value {
        let mut capture = self.capture.borrow_mut();
        unsafe {
            let process: id = msg_send![class!(NSProcessInfo), processInfo];
            let at: f64 = msg_send![process, systemUptime];
            capture.refresh(at);
        }
        snapshot(&capture)
    }

    pub fn cancel(&mut self) {
        self.capture.borrow_mut().cancel();
        self.stop();
    }

    fn stop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        unsafe {
            if !self.monitor.is_null() {
                let _: () = msg_send![class!(NSEvent), removeMonitor: self.monitor];
                let _: () = msg_send![self.monitor, release];
                self.monitor = nil;
            }
            if !self.observer.is_null() {
                let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
                let _: () = msg_send![center, removeObserver: self.observer];
                let _: () = msg_send![self.observer, release];
                self.observer = nil;
            }
        }
        RECORDING.set(false);
    }
}

fn snapshot(capture: &Capture) -> serde_json::Value {
    match &capture.result {
        CaptureResult::Recording => {
            serde_json::json!({ "state": "recording", "preview": capture.preview, "phase": capture.phase(), "ready": false })
        }
        CaptureResult::Recorded(shortcut) => {
            serde_json::json!({ "state": "recorded", "shortcut": shortcut, "phase": capture.phase(), "ready": capture.ready() })
        }
        CaptureResult::Cancelled => serde_json::json!({ "state": "cancelled" }),
        CaptureResult::Error(error) => {
            serde_json::json!({ "state": "error", "error": error, "ready": capture.drained(), "phase": capture.phase() })
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop();
        unsafe {
            let _: () = msg_send![self.window, release];
        }
    }
}
