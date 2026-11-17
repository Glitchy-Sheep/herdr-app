//! A nonactivating panel stays in the current Space, including over full-screen apps.

#![allow(deprecated)]
#![allow(unexpected_cfgs)]

use super::SavedSize;
use block::ConcreteBlock;
use cocoa::appkit::{NSApplication, NSEvent, NSScreen, NSWindow};
use cocoa::base::{id, nil};
use cocoa::foundation::{NSArray, NSPoint, NSRect, NSSize, NSString};
use gpui::Window;
use objc::runtime::{BOOL, NO, YES};
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;

mod recorder;
mod shortcut;
pub use recorder::Recorder;
use shortcut::{Chord, DoubleTap, Modifier, Shortcut};

thread_local! { static RECORDING: Cell<bool> = const { Cell::new(false) }; }

const KEY_DOWN: u64 = 10;
const KEY_UP: u64 = 11;
const FLAGS_CHANGED: u64 = 12;
const INPUT_MASK: u64 = (1 << KEY_DOWN)
    | (1 << KEY_UP)
    | (1 << FLAGS_CHANGED)
    | (1 << 1)
    | (1 << 3)
    | (1 << 25)
    | (1 << 22);
const HOTKEY_SIGNATURE: u32 = u32::from_be_bytes(*b"HdrQ");
const HOTKEY_PRESSED: u32 = 5;
const HOTKEY_RELEASED: u32 = 6;
const HOTKEY_EXCLUSIVE: u32 = 1;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    static kAXTrustedCheckOptionPrompt: *const c_void;
}

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn GetEventKind(event: *mut c_void) -> u32;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32,
        count: u32,
        events: *const EventTypeSpec,
        data: *mut c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn RemoveEventHandler(handler: *mut c_void) -> i32;
    fn RegisterEventHotKey(
        key: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        out: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hotkey: *mut c_void) -> i32;
    fn GetEventParameter(
        event: *mut c_void,
        name: u32,
        kind: u32,
        actual_kind: *mut u32,
        size: u32,
        actual_size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

pub(super) fn permission_foreground() -> super::PermissionForeground {
    use super::PermissionForeground;
    unsafe {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let front: id = msg_send![workspace, frontmostApplication];
        let bundle: id = msg_send![front, bundleIdentifier];
        if bundle.is_null() {
            return PermissionForeground::Other;
        }
        let name = bundle.UTF8String();
        if name.is_null() {
            return PermissionForeground::Other;
        }
        match std::ffi::CStr::from_ptr(name).to_bytes() {
            b"com.apple.systempreferences" => PermissionForeground::Settings,
            b"com.apple.SecurityAgent" | b"com.apple.LocalAuthentication.UIAgent" => {
                PermissionForeground::AuthorizationDialog
            }
            _ => PermissionForeground::Other,
        }
    }
}

pub fn shortcut_needs_accessibility(shortcut: &str) -> bool {
    matches!(Shortcut::parse(shortcut), Ok(Shortcut::Double(_)))
}

pub fn open_permission_settings(panel: Option<&Native>) -> Result<(), String> {
    unsafe {
        let yes: id = msg_send![class!(NSNumber), numberWithBool: YES];
        let options: id = msg_send![class!(NSDictionary), dictionaryWithObject: yes forKey: kAXTrustedCheckOptionPrompt];
        AXIsProcessTrustedWithOptions(options.cast());
        let text = NSString::alloc(nil).init_str(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        );
        let url: id = msg_send![class!(NSURL), URLWithString: text];
        let _: () = msg_send![text, release];
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let opened: BOOL = msg_send![workspace, openURL: url];
        if opened != YES {
            return Err("Open System Settings > Privacy & Security > Accessibility / Device Control and Data Access, then enable Herdr".into());
        }
    }
    if let Some(panel) = panel {
        panel.order_out(FocusAfterHide::SystemSettings);
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusAfterHide {
    PreviousApp,
    SystemSettings,
}

#[derive(Default)]
struct PermissionRecovery {
    lost: bool,
}

impl PermissionRecovery {
    fn observe(&mut self, granted: bool) {
        self.lost |= !granted;
    }
}

fn request(name: &str) {
    crate::mods::request(crate::mods::Request::Run {
        name: name.into(),
        args: serde_json::Value::Null,
    });
}

pub fn validate_shortcut(text: &str) -> Result<String, String> {
    Shortcut::parse(text).map(|shortcut| shortcut.name())
}

thread_local! {
    static KEY_LABELS: RefCell<std::collections::BTreeMap<u16, String>> = RefCell::default();
}

pub fn shortcut_key_label(text: &str) -> Option<String> {
    let Shortcut::Chord(chord) = Shortcut::parse(text).ok()? else {
        return None;
    };
    // Space, Return, Tab and function keys have explicit names in Settings.
    if chord.key > 50 || matches!(chord.key, 36 | 48 | 49) {
        return None;
    }
    KEY_LABELS.with_borrow_mut(|labels| {
        let label = labels
            .entry(chord.key)
            .or_insert_with(|| gpui_macos::shortcut_key_label(chord.key));
        (!label.is_empty() && !label.chars().any(char::is_control)).then(|| label.clone())
    })
}

pub fn keyboard_layout_changed() {
    if KEY_LABELS.with_borrow_mut(|labels| !std::mem::take(labels).is_empty()) {
        crate::mods::request(crate::mods::Request::Refresh);
    }
}

struct State {
    window: id,
    previous_app: Cell<id>,
    width: f32,
    height: f32,
    pending: Cell<Option<bool>>,
    generation: Cell<u64>,
    shortcut_activations: Cell<u64>,
    hotkey_down: Cell<bool>,
    shortcut_suspended: Cell<bool>,
    accessibility_granted: Cell<bool>,
    auto_hide: Cell<bool>,
    resize: RefCell<ResizeMemory>,
}

#[derive(Default)]
struct ResizeMemory {
    preferred: Option<SavedSize>,
    last: Option<SavedSize>,
    placing: bool,
}

impl ResizeMemory {
    fn placed(&mut self, size: SavedSize) {
        self.last = Some(size);
        self.placing = false;
    }

    fn resized(&mut self, size: SavedSize) -> Option<SavedSize> {
        if self.placing || !valid_size(size) {
            return None;
        }
        let previous = self.last.replace(size)?;
        if (previous.width - size.width).abs() < 0.5 && (previous.height - size.height).abs() < 0.5
        {
            return None;
        }
        self.preferred = Some(size);
        Some(size)
    }
}

fn valid_size(size: SavedSize) -> bool {
    size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0
}

fn frame_size(frame: NSRect) -> SavedSize {
    SavedSize {
        width: frame.size.width as f32,
        height: frame.size.height as f32,
    }
}

impl State {
    fn shortcut_pressed(&self) {
        if self.shortcut_suspended.get() || RECORDING.get() {
            return;
        }
        self.shortcut_activations
            .set(self.shortcut_activations.get().saturating_add(1));
        request("quick_terminal.toggle");
    }
}

impl Drop for State {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.window, release];
            let previous = self.previous_app.replace(nil);
            if !previous.is_null() {
                let _: () = msg_send![previous, release];
            }
        }
    }
}

/// AppKit invokes all of these monitors and observers on the main thread.
pub struct Native {
    state: Rc<State>,
    monitors: Vec<id>,
    observer: id,
    carbon_handler: *mut c_void,
    carbon_hotkey: *mut c_void,
    shortcut: Shortcut,
    double_detector: Option<Rc<RefCell<DoubleTap>>>,
    permission: PermissionRecovery,
}

impl Native {
    pub fn new(window: &Window, config: &super::Config) -> Result<Self, String> {
        let shortcut = Shortcut::parse(&config.shortcut)?;
        let view = match HasWindowHandle::window_handle(window)
            .map_err(|e| e.to_string())?
            .as_raw()
        {
            RawWindowHandle::AppKit(handle) => handle.ns_view.as_ptr() as id,
            _ => return Err("Quick terminal needs an AppKit window".into()),
        };
        unsafe {
            let panel: id = msg_send![view, window];
            if panel.is_null() {
                return Err("Quick terminal has no native window".into());
            }
            let is_panel: BOOL = msg_send![panel, isKindOfClass: class!(NSPanel)];
            if is_panel != YES {
                return Err("Enable quick terminal and restart Herdr to create its panel".into());
            }
            let _: id = msg_send![panel, retain];
            // Below menus and system overlays, above ordinary and full-screen windows.
            let _: () = msg_send![panel, setLevel: 22_i64];
            let _: () = msg_send![panel, setHidesOnDeactivate: NO];
            let _: () = msg_send![panel, setBecomesKeyOnlyIfNeeded: NO];
            let _: () = msg_send![panel, setCollectionBehavior: (1_u64 | (1 << 8) | (1 << 12))];
            // Keep fullscreen auxiliary behavior; fullscreen-none is mutually exclusive.
            let zoom_button: id = msg_send![panel, standardWindowButton: 2_u64];
            if !zoom_button.is_null() {
                let _: () = msg_send![zoom_button, setEnabled: NO];
            }
            let _: () = msg_send![panel, setAnimationBehavior: 2_i64];

            let mut native = Self {
                state: Rc::new(State {
                    window: panel,
                    previous_app: Cell::new(nil),
                    width: config.width,
                    height: config.height,
                    pending: Cell::new(None),
                    generation: Cell::new(0),
                    shortcut_activations: Cell::new(0),
                    hotkey_down: Cell::new(false),
                    shortcut_suspended: Cell::new(false),
                    accessibility_granted: Cell::new(false),
                    auto_hide: Cell::new(config.auto_hide),
                    resize: RefCell::new(ResizeMemory::default()),
                }),
                monitors: Vec::new(),
                observer: nil,
                carbon_handler: std::ptr::null_mut(),
                carbon_hotkey: std::ptr::null_mut(),
                shortcut: shortcut.clone(),
                double_detector: None,
                permission: PermissionRecovery::default(),
            };
            match shortcut {
                Shortcut::Double(modifier) => {
                    native.observe_permission(accessibility_trusted());
                    native.install_double_modifier(modifier)?;
                }
                Shortcut::Chord(chord) => native.install_chord(&chord)?,
            }
            native.install_blur();
            Ok(native)
        }
    }

    unsafe fn install_double_modifier(&mut self, modifier: Modifier) -> Result<(), String> {
        let detector = Rc::new(RefCell::new(DoubleTap::new(modifier)));
        self.double_detector = Some(detector.clone());
        let state = Rc::downgrade(&self.state);
        let observe = move |event: id, global: bool| {
            unsafe {
                let kind: u64 = msg_send![event, type];
                let mut detector = detector.borrow_mut();
                let Some(state) = state.upgrade() else { return };
                // macOS can omit intervening key presses from an untrusted global monitor.
                if state.shortcut_suspended.get()
                    || RECORDING.get()
                    || (global && !state.accessibility_granted.get())
                {
                    *detector = DoubleTap::new(modifier);
                    return;
                }
                if kind == KEY_DOWN || kind == KEY_UP {
                    let key: u16 = msg_send![event, keyCode];
                    detector.ordinary_key(key, kind == KEY_DOWN);
                    return;
                }
                if kind != FLAGS_CHANGED {
                    detector.cancel();
                    return;
                }
                let flags: u64 = msg_send![event, modifierFlags];
                let key: u16 = msg_send![event, keyCode];
                let at: f64 = msg_send![event, timestamp];
                if detector.flags(key, flags, at) && (!global || accessibility_trusted()) {
                    state.shortcut_pressed();
                }
            }
        };
        unsafe {
            let global_observe = observe.clone();
            let global = ConcreteBlock::new(move |event: id| global_observe(event, true)).copy();
            let monitor: id = msg_send![class!(NSEvent), addGlobalMonitorForEventsMatchingMask: INPUT_MASK handler: &*global];
            if monitor.is_null() {
                return Err("macOS could not install the quick-terminal key monitor".into());
            }
            let _: id = msg_send![monitor, retain];
            self.monitors.push(monitor);

            let local = ConcreteBlock::new(move |event: id| {
                observe(event, false);
                event
            })
            .copy();
            let monitor: id = msg_send![class!(NSEvent), addLocalMonitorForEventsMatchingMask: INPUT_MASK handler: &*local];
            if monitor.is_null() {
                return Err("macOS could not install the local quick-terminal key monitor".into());
            }
            let _: id = msg_send![monitor, retain];
            self.monitors.push(monitor);
        }
        if !self.state.accessibility_granted.get() {
            tracing::warn!(
                "Double modifier taps need Accessibility / Device Control and Data Access: System Settings > Privacy & Security > Accessibility / Device Control and Data Access > Herdr. A regular shortcut such as ctrl-alt-space works without this permission."
            );
        }
        Ok(())
    }

    unsafe fn install_chord(&mut self, chord: &Chord) -> Result<(), String> {
        unsafe {
            let target = GetApplicationEventTarget();
            let events = [
                EventTypeSpec {
                    class: u32::from_be_bytes(*b"keyb"),
                    kind: HOTKEY_PRESSED,
                },
                EventTypeSpec {
                    class: u32::from_be_bytes(*b"keyb"),
                    kind: HOTKEY_RELEASED,
                },
            ];
            let result = InstallEventHandler(
                target,
                hotkey_event,
                events.len() as u32,
                events.as_ptr(),
                Rc::as_ptr(&self.state) as *mut c_void,
                &mut self.carbon_handler,
            );
            if result != 0 {
                return Err(format!(
                    "macOS could not install the hotkey handler ({result})"
                ));
            }
            self.register_chord(chord)?;
        }
        Ok(())
    }

    unsafe fn register_chord(&mut self, chord: &Chord) -> Result<(), String> {
        unsafe {
            let result = RegisterEventHotKey(
                u32::from(chord.key),
                chord.carbon_flags(),
                EventHotKeyId {
                    signature: HOTKEY_SIGNATURE,
                    id: 1,
                },
                GetApplicationEventTarget(),
                HOTKEY_EXCLUSIVE,
                &mut self.carbon_hotkey,
            );
            if result != 0 {
                let shortcut = chord.name();
                return Err(format!(
                    "Cannot register quick-terminal shortcut '{shortcut}' ({result}); it may already be in use"
                ));
            }
        }
        Ok(())
    }

    pub fn set_shortcut_suspended(&mut self, suspended: bool) -> Result<(), String> {
        if self.state.shortcut_suspended.get() == suspended {
            return Ok(());
        }
        if let Some(detector) = &self.double_detector {
            let Shortcut::Double(modifier) = self.shortcut else {
                unreachable!()
            };
            *detector.borrow_mut() = DoubleTap::new(modifier);
        }
        if suspended {
            if !self.carbon_hotkey.is_null() {
                let result = unsafe { UnregisterEventHotKey(self.carbon_hotkey) };
                if result != 0 {
                    return Err(format!(
                        "Cannot suspend the quick-terminal shortcut ({result})"
                    ));
                }
                self.carbon_hotkey = std::ptr::null_mut();
            }
        } else if let Shortcut::Chord(chord) = self.shortcut.clone() {
            unsafe {
                self.register_chord(&chord)?;
            }
        }
        self.state.hotkey_down.set(false);
        self.state.shortcut_suspended.set(suspended);
        Ok(())
    }

    pub fn set_auto_hide(&mut self, auto_hide: bool) {
        self.state.auto_hide.set(auto_hide);
    }

    unsafe fn install_blur(&mut self) {
        let state = Rc::downgrade(&self.state);
        let block = ConcreteBlock::new(move |_notification: id| {
            let state = state.clone();
            // AppKit settles the next key window after delivering resign-key notifications.
            let later = ConcreteBlock::new(move || {
                let Some(state) = state.upgrade() else { return };
                if !state.auto_hide.get() {
                    return;
                }
                unsafe {
                    let visible: BOOL = msg_send![state.window, isVisible];
                    let key: BOOL = msg_send![state.window, isKeyWindow];
                    let app = NSApplication::sharedApplication(nil);
                    let other_key: id = msg_send![app, keyWindow];
                    if visible == YES && key == NO && other_key.is_null() {
                        crate::mods::request(crate::mods::Request::QuickTerminalAutoHide);
                    }
                }
            })
            .copy();
            unsafe {
                let queue: id = msg_send![class!(NSOperationQueue), mainQueue];
                let _: () = msg_send![queue, addOperationWithBlock: &*later];
            }
        })
        .copy();
        unsafe {
            let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
            let name = NSString::alloc(nil).init_str("NSWindowDidResignKeyNotification");
            let observer: id = msg_send![center, addObserverForName: name object: self.state.window queue: nil usingBlock: &*block];
            let _: () = msg_send![name, release];
            if !observer.is_null() {
                let _: id = msg_send![observer, retain];
                self.observer = observer;
            }
        }
    }

    pub fn show(&mut self) -> Result<(), String> {
        unsafe {
            let key: BOOL = msg_send![self.state.window, isKeyWindow];
            if self.state.pending.get() == Some(true) || (self.is_visible() && key == YES) {
                return Ok(());
            }
            let screens = NSScreen::screens(nil);
            let pointer = NSEvent::mouseLocation(nil);
            let mut screen = NSScreen::mainScreen(nil);
            for index in 0..screens.count() {
                let candidate = screens.objectAtIndex(index);
                let frame = NSScreen::frame(candidate);
                if contains(frame, pointer) {
                    screen = candidate;
                    break;
                }
            }
            if screen.is_null() {
                return Err("No display is available for the quick terminal".into());
            }
            let frame: NSRect = msg_send![screen, visibleFrame];
            let bounds = placement(
                frame,
                self.state.width,
                self.state.height,
                self.state.resize.borrow().preferred,
            );
            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            let front: id = msg_send![workspace, frontmostApplication];
            if !front.is_null() {
                let pid: i32 = msg_send![front, processIdentifier];
                if pid != std::process::id() as i32 {
                    let _: id = msg_send![front, retain];
                    let previous = self.state.previous_app.replace(front);
                    if !previous.is_null() {
                        let _: () = msg_send![previous, release];
                    }
                }
            }
            self.schedule(true, move |state| {
                state.resize.borrow_mut().placing = true;
                let _: () = msg_send![state.window, herdrSetQuickResizeBounds: frame];
                let app = NSApplication::sharedApplication(nil);
                let _: () = msg_send![app, unhideWithoutActivation];
                let minimized: BOOL = msg_send![state.window, isMiniaturized];
                if minimized == YES {
                    let _: () = msg_send![state.window, deminiaturize: nil];
                }
                state.window.setFrame_display_(bounds, YES);
                state.window.makeKeyAndOrderFront_(nil);
                let actual: NSRect = msg_send![state.window, frame];
                state.resize.borrow_mut().placed(frame_size(actual));
            });
        }
        Ok(())
    }

    pub fn set_saved_size(&mut self, size: Option<SavedSize>) {
        self.state.resize.borrow_mut().preferred = size.filter(|size| valid_size(*size));
    }

    pub fn resized(&mut self) -> Option<SavedSize> {
        if self.state.pending.get().is_some() || !self.is_visible() {
            return None;
        }
        let frame: NSRect = unsafe { msg_send![self.state.window, frame] };
        self.state.resize.borrow_mut().resized(frame_size(frame))
    }

    pub fn hide(&mut self) -> Result<(), String> {
        if self.state.pending.get() != Some(false) {
            self.order_out(FocusAfterHide::PreviousApp);
        }
        Ok(())
    }

    fn order_out(&self, focus: FocusAfterHide) {
        self.schedule(false, move |state| unsafe {
            let was_key: BOOL = msg_send![state.window, isKeyWindow];
            let _: () = msg_send![state.window, orderOut: nil];
            let previous = state.previous_app.replace(nil);
            if !previous.is_null() {
                let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
                let front: id = msg_send![workspace, frontmostApplication];
                let front_pid: i32 = if front.is_null() {
                    0
                } else {
                    msg_send![front, processIdentifier]
                };
                // A nonactivating panel normally leaves the previous app active already.
                if focus == FocusAfterHide::PreviousApp
                    && was_key == YES
                    && front_pid == std::process::id() as i32
                {
                    let _: BOOL = msg_send![previous, activateWithOptions: 0_u64];
                }
                let _: () = msg_send![previous, release];
            }
        });
    }

    pub fn is_visible(&self) -> bool {
        if let Some(visible) = self.state.pending.get() {
            return visible;
        }
        unsafe {
            let visible: BOOL = msg_send![self.state.window, isVisible];
            let minimized: BOOL = msg_send![self.state.window, isMiniaturized];
            let app = NSApplication::sharedApplication(nil);
            let hidden: BOOL = msg_send![app, isHidden];
            visible == YES && minimized == NO && hidden == NO
        }
    }

    pub fn uses_double_modifier(&self) -> bool {
        matches!(self.shortcut, Shortcut::Double(_))
    }

    pub fn warning(&self, granted: bool) -> Option<&'static str> {
        (self.uses_double_modifier() && !granted).then_some(
            "Double modifier taps need Accessibility / Device Control and Data Access in System Settings > Privacy & Security. Enable Herdr there and restart it, or choose a regular shortcut such as ctrl-alt-space.",
        )
    }

    pub fn observe_permission(&mut self, granted: bool) {
        if self.uses_double_modifier() {
            self.state.accessibility_granted.set(granted);
            self.permission.observe(granted);
        }
    }

    pub fn permission_restart_required(&self) -> bool {
        self.permission.lost
    }

    pub fn shortcut_activations(&self) -> u64 {
        self.state.shortcut_activations.get()
    }

    pub fn diagnostics(&self) -> serde_json::Value {
        unsafe {
            let visible: BOOL = msg_send![self.state.window, isVisible];
            let key: BOOL = msg_send![self.state.window, isKeyWindow];
            let on_active_space: BOOL = msg_send![self.state.window, isOnActiveSpace];
            let frame: NSRect = msg_send![self.state.window, frame];
            let app = NSApplication::sharedApplication(nil);
            let application_active: BOOL = msg_send![app, isActive];
            let application_hidden: BOOL = msg_send![app, isHidden];
            let window_level: i64 = msg_send![self.state.window, level];
            let hides_on_deactivate: BOOL = msg_send![self.state.window, hidesOnDeactivate];
            let centered_resize_samples: u64 =
                msg_send![self.state.window, herdrQuickResizeSamples];
            serde_json::json!({
                "visible": visible == YES,
                "key": key == YES,
                "on_active_space": on_active_space == YES,
                "application_active": application_active == YES,
                "application_hidden": application_hidden == YES,
                "auto_hide": self.state.auto_hide.get(),
                "window_level": window_level,
                "hides_on_deactivate": hides_on_deactivate == YES,
                "pending_visibility": self.state.pending.get(),
                "frame": { "x": frame.origin.x, "y": frame.origin.y,
                    "width": frame.size.width, "height": frame.size.height },
                "shortcut_activations": self.shortcut_activations(),
                "centered_resize_samples": centered_resize_samples,
            })
        }
    }

    // Native window notifications must run outside GPUI's current window update.
    fn schedule(&self, visible: bool, action: impl Fn(&State) + 'static) {
        let generation = self.state.generation.get().wrapping_add(1);
        self.state.generation.set(generation);
        self.state.pending.set(Some(visible));
        let state = Rc::downgrade(&self.state);
        let block = ConcreteBlock::new(move || {
            let Some(state) = state.upgrade() else { return };
            if state.generation.get() != generation {
                return;
            }
            action(&state);
            state.pending.set(None);
        })
        .copy();
        unsafe {
            let queue: id = msg_send![class!(NSOperationQueue), mainQueue];
            let _: () = msg_send![queue, addOperationWithBlock: &*block];
        }
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.state.window, herdrClearQuickResize];
            for monitor in self.monitors.drain(..) {
                let _: () = msg_send![class!(NSEvent), removeMonitor: monitor];
                let _: () = msg_send![monitor, release];
            }
            if !self.observer.is_null() {
                let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
                let _: () = msg_send![center, removeObserver: self.observer];
                let _: () = msg_send![self.observer, release];
            }
            if !self.carbon_hotkey.is_null() {
                UnregisterEventHotKey(self.carbon_hotkey);
            }
            if !self.carbon_handler.is_null() {
                RemoveEventHandler(self.carbon_handler);
            }
        }
    }
}

unsafe extern "C" fn hotkey_event(
    _next: *mut c_void,
    event: *mut c_void,
    data: *mut c_void,
) -> i32 {
    let mut hotkey = EventHotKeyId::default();
    let result = unsafe {
        GetEventParameter(
            event,
            u32::from_be_bytes(*b"----"),
            u32::from_be_bytes(*b"hkid"),
            std::ptr::null_mut(),
            std::mem::size_of::<EventHotKeyId>() as u32,
            std::ptr::null_mut(),
            (&mut hotkey as *mut EventHotKeyId).cast(),
        )
    };
    if result == 0 && hotkey.signature == HOTKEY_SIGNATURE && hotkey.id == 1 && !data.is_null() {
        // Native removes the handler before releasing its state.
        let state = unsafe { &*data.cast::<State>() };
        match unsafe { GetEventKind(event) } {
            HOTKEY_PRESSED if !state.hotkey_down.replace(true) => state.shortcut_pressed(),
            HOTKEY_RELEASED => state.hotkey_down.set(false),
            _ => {}
        }
        0
    } else {
        -9874 // eventNotHandledErr
    }
}

fn contains(frame: NSRect, point: NSPoint) -> bool {
    point.x >= frame.origin.x
        && point.y >= frame.origin.y
        && point.x < frame.origin.x + frame.size.width
        && point.y < frame.origin.y + frame.size.height
}

fn placement(frame: NSRect, width: f32, height: f32, saved: Option<SavedSize>) -> NSRect {
    let (width, height) = match saved {
        Some(size) if valid_size(size) => (f64::from(size.width), f64::from(size.height)),
        _ => (
            frame.size.width * f64::from(width),
            frame.size.height * f64::from(height),
        ),
    };
    let width = width.min(frame.size.width);
    let height = height.min(frame.size.height);
    NSRect::new(
        NSPoint::new(
            frame.origin.x + (frame.size.width - width) / 2.0,
            frame.origin.y + frame.size.height - height,
        ),
        NSSize::new(width, height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_modifier_shortcuts_need_accessibility() {
        for shortcut in [
            "double-cmd",
            "double-left-alt",
            "double-right-shift",
            " DOUBLE-ROPTION ",
        ] {
            assert!(shortcut_needs_accessibility(shortcut));
        }
        for shortcut in ["ctrl-alt-space", "f8", "double-invalid", ""] {
            assert!(!shortcut_needs_accessibility(shortcut));
        }
    }

    #[test]
    fn accessibility_loss_requires_a_fresh_monitor_even_after_access_returns() {
        let mut permission = PermissionRecovery::default();
        permission.observe(true);
        assert!(!permission.lost);
        permission.observe(false);
        permission.observe(true);
        assert!(permission.lost);
        let mut restarted = PermissionRecovery::default();
        restarted.observe(true);
        assert!(!restarted.lost);
    }

    #[test]
    fn starting_without_access_requires_restart_after_grant() {
        let mut permission = PermissionRecovery::default();
        permission.observe(false);
        permission.observe(false);
        permission.observe(true);
        assert!(permission.lost);
    }

    #[test]
    fn placement_uses_the_selected_displays_visible_top_edge() {
        let bounds = placement(
            NSRect::new(NSPoint::new(-1920.0, 40.0), NSSize::new(1920.0, 1040.0)),
            0.8,
            0.5,
            None,
        );
        assert!((bounds.origin.x - -1728.0).abs() < 0.001);
        assert_eq!(bounds.origin.y, 560.0);
        assert_eq!(bounds.size.height, 520.0);
    }

    #[test]
    fn saved_size_is_clamped_for_a_small_display_and_restored_on_a_large_one() {
        let preferred = SavedSize {
            width: 1600.0,
            height: 1000.0,
        };
        let small = NSRect::new(NSPoint::new(0.0, 40.0), NSSize::new(1200.0, 800.0));
        let large = NSRect::new(NSPoint::new(-1920.0, 40.0), NSSize::new(1920.0, 1040.0));
        let shown = placement(small, 0.8, 0.5, Some(preferred));
        assert_eq!(
            frame_size(shown),
            SavedSize {
                width: 1200.0,
                height: 800.0
            }
        );
        let restored = placement(large, 0.8, 0.5, Some(preferred));
        assert_eq!(frame_size(restored), preferred);
        assert_eq!(restored.origin.x, -1760.0);
        assert_eq!(restored.origin.y, 80.0);
    }

    #[test]
    fn placement_callbacks_do_not_replace_the_preferred_size() {
        let preferred = SavedSize {
            width: 1600.0,
            height: 1000.0,
        };
        let clamped = SavedSize {
            width: 1200.0,
            height: 800.0,
        };
        let manual = SavedSize {
            width: 1100.0,
            height: 700.0,
        };
        let mut memory = ResizeMemory {
            preferred: Some(preferred),
            last: Some(preferred),
            placing: true,
        };
        assert_eq!(memory.resized(clamped), None);
        memory.placed(clamped);
        assert_eq!(memory.resized(clamped), None);
        assert_eq!(memory.preferred, Some(preferred));
        assert_eq!(memory.resized(manual), Some(manual));
        assert_eq!(memory.preferred, Some(manual));
        assert_eq!(memory.resized(manual), None);
    }
}
