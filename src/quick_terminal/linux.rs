//! X11 global shortcuts and managed EWMH windows. Wayland needs compositor support.

#[path = "chord.rs"]
mod chord;
#[path = "linux/keyboard.rs"]
mod keyboard;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::{
    Event,
    randr::ConnectionExt as _,
    xkb::{BoolCtrl, ConnectionExt as _, PerClientFlag, StatePart},
    xproto::*,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

type Conn = Arc<RustConnection>;

pub fn validate_shortcut(text: &str) -> Result<(), String> {
    chord::parse(text).map(|_| ())
}

#[derive(Default)]
pub struct KeyNames {
    connection: Option<RustConnection>,
    names: std::collections::HashMap<String, String>,
}

impl KeyNames {
    pub fn new() -> Result<Self, String> {
        let (connection, _) = x11rb::connect(None).map_err(|error| {
            format!("Cannot read the X11 keyboard mapping for shortcut recording: {error}")
        })?;
        let extension = connection
            .xkb_use_extension(1, 0)
            .map_err(error)?
            .reply()
            .map_err(error)?;
        if !extension.supported {
            return Err("The shortcut recorder requires XKB".into());
        }
        let layout = keyboard::KeyboardMap::read(&connection)?;
        let mut names = std::collections::HashMap::new();
        for symbols in layout.symbols() {
            let characters: Vec<_> = symbols
                .iter()
                .filter_map(|&symbol| {
                    let unicode = if symbol <= 0xff {
                        symbol
                    } else if symbol & 0xff000000 == 0x01000000 {
                        symbol & 0xffffff
                    } else {
                        return None;
                    };
                    char::from_u32(unicode).filter(|character| !character.is_control())
                })
                .collect();
            let Some(base) = characters
                .iter()
                .find(|character| character.is_ascii_alphanumeric())
                .or(characters.first())
            else {
                continue;
            };
            for character in &characters {
                names
                    .entry(character.to_lowercase().to_string())
                    .or_insert_with(|| base.to_lowercase().to_string());
            }
        }
        Ok(Self {
            connection: Some(connection),
            names,
        })
    }

    pub fn normalize(&self, key: &str) -> String {
        let key = key.to_lowercase();
        // Layout groups can put different letters on the same key; keep their identities separate.
        if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() {
            return key;
        }
        self.names.get(&key).cloned().unwrap_or(key)
    }

    pub fn keyboard_released(&self) -> Result<bool, String> {
        let connection = self
            .connection
            .as_ref()
            .ok_or("The shortcut recorder is not active")?;
        Ok(connection
            .query_keymap()
            .map_err(error)?
            .reply()
            .map_err(error)?
            .keys
            .iter()
            .all(|byte| *byte == 0))
    }
}

struct Hotkey {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Hotkey {
    fn new(
        connection: Conn,
        root: u32,
        window: u32,
        shortcut: &str,
        auto_hide: Arc<AtomicBool>,
        timestamp: Arc<AtomicU32>,
        recenter: Arc<Mutex<Option<Rect>>>,
    ) -> Result<Self, String> {
        let chord = chord::parse(shortcut)?;
        let extension = connection
            .xkb_use_extension(1, 0)
            .map_err(error)?
            .reply()
            .map_err(error)?;
        if !extension.supported {
            return Err("quick terminal requires the X11 XKB extension".into());
        }
        let flags = connection
            .xkb_per_client_flags(
                0x100,
                PerClientFlag::DETECTABLE_AUTO_REPEAT,
                PerClientFlag::DETECTABLE_AUTO_REPEAT,
                BoolCtrl::default(),
                BoolCtrl::default(),
                BoolCtrl::default(),
            )
            .map_err(error)?
            .reply()
            .map_err(error)?;
        if !flags.value.contains(PerClientFlag::DETECTABLE_AUTO_REPEAT) {
            return Err("X11 cannot suppress global shortcut auto-repeat".into());
        }
        keyboard::watch_layout(&connection)?;
        let mut grabs = keyboard::Grabs::default();
        grabs.update(&connection, root, &chord)?;
        let active_atom = atom(&connection, "_NET_ACTIVE_WINDOW")?;
        let mut was_active = active_window(&connection, root)? == window;
        connection
            .change_window_attributes(
                root,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .map_err(error)?
            .check()
            .map_err(error)?;
        connection.flush().map_err(error)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let shortcut = shortcut.to_owned();
        let thread = std::thread::Builder::new().name("herdr-quick-hotkey".into()).spawn(move || {
            let mut held = None;
            let mut registration_error = None;
            let mut retry_at = None;
            while !stopped.load(Ordering::Relaxed) {
                match connection.poll_for_event() {
                    Ok(Some(Event::KeyPress(event))) if grabs.matches(&event) => {
                        if held.is_none() {
                            held = Some(event.detail);
                            timestamp.store(event.time, Ordering::Relaxed);
                            if !crate::quick_terminal::record_shortcut(&shortcut) {
                                crate::mods::request(crate::mods::Request::Run {
                                    name: "quick_terminal.toggle".into(), args: serde_json::Value::Null,
                                });
                            }
                        }
                    }
                    Ok(Some(Event::KeyRelease(event))) if held == Some(event.detail) => held = None,
                    Ok(Some(Event::PropertyNotify(event))) if event.window == root && event.atom == active_atom => {
                        let Ok(active) = active_window(&connection, root) else { continue };
                        if active == window {
                            was_active = true;
                        } else if was_active && herdr_window(&connection, active, window) == Ok(false) {
                            was_active = false;
                            if auto_hide.load(Ordering::SeqCst) {
                                crate::mods::request(crate::mods::Request::QuickTerminalAutoHide);
                            }
                        }
                    }
                    Ok(Some(Event::XkbMapNotify(_) | Event::XkbNewKeyboardNotify(_))) => {
                        retry_at = Some(std::time::Instant::now());
                    }
                    Ok(Some(Event::XkbStateNotify(event))) if event.changed.contains(StatePart::GROUP_STATE) => {
                        retry_at = Some(std::time::Instant::now());
                    }
                    Ok(Some(_)) => {},
                    Ok(None) => {
                        let mut pending = recenter.lock().unwrap();
                        if let Some(area) = *pending {
                            match recenter_after_drag(&connection, root, window, area) {
                                Ok(true) => *pending = None,
                                Ok(false) => {},
                                Err(error) => {
                                    *pending = None;
                                    tracing::warn!("quick terminal resize placement: {error}");
                                    crate::mods::request(crate::mods::Request::OpenOverlay {
                                        id: crate::mods::new_overlay_id(),
                                        spec: serde_json::json!({ "kind": "toast", "notification": { "title": "Quick terminal", "body": error, "level": "error" } }),
                                    });
                                }
                            }
                        }
                        drop(pending);
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(error) => {
                        let message = format!("Quick terminal X11 shortcut listener stopped: {error}");
                        tracing::warn!("{message}");
                        crate::mods::request(crate::mods::Request::QuickTerminalShortcutError(Some(message)));
                        break;
                    }
                }
                if retry_at.is_some_and(|retry| std::time::Instant::now() >= retry) {
                    if let Some(key) = held
                        && connection.query_keymap().ok().and_then(|cookie| cookie.reply().ok())
                            .is_some_and(|map| map.keys[usize::from(key / 8)] & (1 << (key % 8)) == 0)
                    {
                        held = None;
                    }
                    match grabs.update(&connection, root, &chord) {
                        Ok(()) => {
                            if registration_error.take().is_some() {
                                crate::mods::request(crate::mods::Request::QuickTerminalShortcutError(None));
                            }
                            retry_at = None;
                        }
                        Err(error) => {
                            if registration_error.as_ref() != Some(&error) {
                                crate::mods::request(crate::mods::Request::QuickTerminalShortcutError(Some(error.clone())));
                                crate::mods::request(crate::mods::Request::OpenOverlay {
                                    id: crate::mods::new_overlay_id(),
                                    spec: serde_json::json!({ "kind": "toast", "notification": { "title": "Quick terminal shortcut unavailable", "body": &error, "level": "error" } }),
                                });
                            }
                            registration_error = Some(error);
                            retry_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                        }
                    }
                }
            }
            grabs.clear(&connection, root);
        }).map_err(|e| format!("cannot start quick terminal shortcut listener: {e}"))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Hotkey {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct Native {
    connection: Conn,
    root: u32,
    window: u32,
    previous: u32,
    width: f32,
    height: f32,
    saved_size: Option<super::SavedSize>,
    last_size: Option<(u16, u16)>,
    programmatic_size: Option<((u16, u16), (u16, u16))>,
    anchor: Option<Rect>,
    recenter: Arc<Mutex<Option<Rect>>>,
    timestamp: Arc<AtomicU32>,
    auto_hide: Arc<AtomicBool>,
    _hotkey: Hotkey,
}

impl Native {
    pub fn keyboard_released(&self) -> Result<bool, String> {
        let keys = self
            .connection
            .query_keymap()
            .map_err(error)?
            .reply()
            .map_err(error)?
            .keys;
        Ok(keys.iter().all(|byte| *byte == 0))
    }

    pub fn new(window: &gpui::Window, config: &super::Config) -> Result<Self, String> {
        let window = match HasWindowHandle::window_handle(window).map_err(|e| e.to_string())?.as_raw() {
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).map_err(error)?,
            RawWindowHandle::Wayland(_) => return Err("Native quick terminal is not available on Wayland: global shortcuts, placement and activation require compositor support. Disable quick_terminal.enabled and use your compositor's window/scratchpad shortcut; an X11 session supports the built-in mode.".into()),
            _ => return Err("quick terminal requires an X11 window".into()),
        };
        let (connection, _) = x11rb::connect(None).map_err(error)?;
        let connection = Arc::new(connection);
        let root = connection
            .get_geometry(window)
            .map_err(error)?
            .reply()
            .map_err(error)?
            .root;
        let supported = property(&connection, root, "_NET_SUPPORTED", AtomEnum::ATOM.into())?;
        for name in [
            "_NET_ACTIVE_WINDOW",
            "_NET_WM_STATE",
            "_NET_WM_STATE_ABOVE",
            "_NET_WM_DESKTOP",
        ] {
            if !supported.contains(&atom(&connection, name)?) {
                return Err(format!(
                    "quick terminal requires an EWMH window manager supporting {name}"
                ));
            }
        }
        let timestamp = Arc::new(AtomicU32::new(x11rb::CURRENT_TIME));
        let recenter = Arc::new(Mutex::new(None));
        let auto_hide = Arc::new(AtomicBool::new(config.auto_hide));
        let hotkey = Hotkey::new(
            connection.clone(),
            root,
            window,
            &config.shortcut,
            auto_hide.clone(),
            timestamp.clone(),
            recenter.clone(),
        )?;
        Ok(Self {
            connection,
            root,
            window,
            previous: 0,
            width: config.width,
            height: config.height,
            saved_size: None,
            last_size: None,
            programmatic_size: None,
            anchor: None,
            recenter,
            timestamp,
            auto_hide,
            _hotkey: hotkey,
        })
    }

    pub fn set_auto_hide(&mut self, enabled: bool) {
        self.auto_hide.store(enabled, Ordering::SeqCst);
    }

    /// X11 sizes use native pixels; preferences are scoped to this platform.
    pub fn set_saved_size(&mut self, size: Option<super::SavedSize>) {
        self.saved_size = size.filter(|size| {
            size.width.is_finite()
                && size.height.is_finite()
                && size.width > 0.0
                && size.height > 0.0
        });
    }

    pub fn resized(&mut self) -> Option<super::SavedSize> {
        if !self.is_visible() {
            return None;
        }
        let geometry = self
            .connection
            .get_geometry(self.window)
            .ok()?
            .reply()
            .ok()?;
        let current = (geometry.width, geometry.height);
        if let Some((before, target)) = self.programmatic_size {
            // The window manager can answer a size query before applying the placement.
            if current == before && current != target {
                return None;
            }
            self.programmatic_size = None;
            if current == target {
                self.last_size = Some(current);
                return None;
            }
        }
        if self.last_size.is_none() || self.last_size == Some(current) {
            return None;
        }
        self.last_size = Some(current);
        let size = super::SavedSize {
            width: current.0 as f32,
            height: current.1 as f32,
        };
        self.saved_size = Some(size);
        *self.recenter.lock().unwrap() = self.anchor;
        Some(size)
    }

    pub fn show(&mut self) -> Result<(), String> {
        *self.recenter.lock().unwrap() = None;
        let previous = active_window(&self.connection, self.root)?;
        if previous != 0 && previous != self.window {
            self.previous = previous;
        }
        let pointer = self
            .connection
            .query_pointer(self.root)
            .map_err(error)?
            .reply()
            .map_err(error)?;
        let geometry = self
            .connection
            .get_geometry(self.root)
            .map_err(error)?
            .reply()
            .map_err(error)?;
        let mut area = Rect {
            x: 0,
            y: 0,
            width: geometry.width as i32,
            height: geometry.height as i32,
        };
        if let Ok(reply) = self
            .connection
            .randr_get_monitors(self.root, true)
            .map_err(error)
            .and_then(|cookie| cookie.reply().map_err(error))
            && let Some(monitor) = reply.monitors.iter().find(|monitor| {
                pointer.root_x >= monitor.x
                    && pointer.root_y >= monitor.y
                    && (pointer.root_x as i32) < monitor.x as i32 + monitor.width as i32
                    && (pointer.root_y as i32) < monitor.y as i32 + monitor.height as i32
            })
        {
            area = Rect {
                x: monitor.x as i32,
                y: monitor.y as i32,
                width: monitor.width as i32,
                height: monitor.height as i32,
            };
        }
        let desktop = property(
            &self.connection,
            self.root,
            "_NET_CURRENT_DESKTOP",
            AtomEnum::CARDINAL.into(),
        )?
        .first()
        .copied()
        .unwrap_or(0);
        let workareas = property(
            &self.connection,
            self.root,
            "_NET_WORKAREA",
            AtomEnum::CARDINAL.into(),
        )?;
        if let Some(values) = workareas.get(desktop as usize * 4..desktop as usize * 4 + 4) {
            area = area
                .intersection(Rect {
                    x: values[0] as i32,
                    y: values[1] as i32,
                    width: values[2] as i32,
                    height: values[3] as i32,
                })
                .unwrap_or(area);
        }
        let rect = if let Some(size) = self.saved_size {
            area.with_size(size.width.round() as i32, size.height.round() as i32)
        } else {
            area.top(self.width, self.height)
        };
        let geometry = self
            .connection
            .get_geometry(self.window)
            .map_err(error)?
            .reply()
            .map_err(error)?;
        let before = (geometry.width, geometry.height);
        let target = (rect.width as u16, rect.height as u16);
        let desktop_atom = atom(&self.connection, "_NET_WM_DESKTOP")?;
        let above = atom(&self.connection, "_NET_WM_STATE_ABOVE")?;
        let state_atom = atom(&self.connection, "_NET_WM_STATE")?;
        // An unmapped window's initial properties are read when it is managed again.
        if !self.is_visible() {
            self.connection
                .change_property32(
                    PropMode::REPLACE,
                    self.window,
                    desktop_atom,
                    AtomEnum::CARDINAL,
                    &[desktop],
                )
                .map_err(error)?
                .check()
                .map_err(error)?;
            let mut states = property(
                &self.connection,
                self.window,
                "_NET_WM_STATE",
                AtomEnum::ATOM.into(),
            )?;
            if !states.contains(&above) {
                states.push(above);
            }
            self.connection
                .change_property32(
                    PropMode::REPLACE,
                    self.window,
                    state_atom,
                    AtomEnum::ATOM,
                    &states,
                )
                .map_err(error)?
                .check()
                .map_err(error)?;
        }
        self.connection
            .configure_window(
                self.window,
                &ConfigureWindowAux::new()
                    .x(rect.x)
                    .y(rect.y)
                    .width(rect.width as u32)
                    .height(rect.height as u32),
            )
            .map_err(error)?
            .check()
            .map_err(error)?;
        self.last_size = Some(before);
        self.programmatic_size = (before != target).then_some((before, target));
        self.anchor = Some(area);
        self.connection
            .map_window(self.window)
            .map_err(error)?
            .check()
            .map_err(error)?;
        self.message(self.window, desktop_atom, [desktop, 1, 0, 0, 0])?;
        self.message(self.window, state_atom, [1, above, 0, 1, 0])?;
        self.message(
            self.window,
            atom(&self.connection, "_NET_ACTIVE_WINDOW")?,
            [
                1,
                self.timestamp.swap(x11rb::CURRENT_TIME, Ordering::Relaxed),
                self.previous,
                0,
                0,
            ],
        )?;
        self.connection.flush().map_err(error)?;
        // A managed MapWindow can still be waiting for the window manager.
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut delay = Duration::from_millis(1);
        while !self.viewable()? {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("The X11 window manager did not show the quick terminal".into());
            }
            std::thread::sleep(delay.min(remaining));
            delay = (delay * 2).min(Duration::from_millis(16));
        }
        Ok(())
    }

    pub fn hide(&mut self) -> Result<(), String> {
        self.unmap(active_window(&self.connection, self.root)?)
    }

    pub fn hide_on_blur(&mut self) -> Result<(), String> {
        let active = active_window(&self.connection, self.root)?;
        if self.auto_hide.load(Ordering::SeqCst)
            && !herdr_window(&self.connection, active, self.window)?
        {
            self.unmap(active)?;
        }
        Ok(())
    }

    fn unmap(&mut self, active: u32) -> Result<(), String> {
        *self.recenter.lock().unwrap() = None;
        self.connection
            .unmap_window(self.window)
            .map_err(error)?
            .check()
            .map_err(error)?;
        if active == self.window
            && self.previous != 0
            && self
                .connection
                .get_window_attributes(self.previous)
                .map_err(error)
                .and_then(|c| c.reply().map_err(error))
                .is_ok()
        {
            self.message(
                self.previous,
                atom(&self.connection, "_NET_ACTIVE_WINDOW")?,
                [
                    1,
                    self.timestamp.swap(x11rb::CURRENT_TIME, Ordering::Relaxed),
                    self.window,
                    0,
                    0,
                ],
            )?;
        }
        self.connection.flush().map_err(error)
    }

    pub fn is_visible(&self) -> bool {
        self.viewable().unwrap_or(false)
    }

    fn viewable(&self) -> Result<bool, String> {
        let attributes = self
            .connection
            .get_window_attributes(self.window)
            .map_err(error)?
            .reply()
            .map_err(error)?;
        Ok(attributes.map_state == MapState::VIEWABLE)
    }

    fn message(&self, window: u32, kind: u32, data: [u32; 5]) -> Result<(), String> {
        let message = ClientMessageEvent::new(32, window, kind, data);
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                message,
            )
            .map_err(error)?
            .check()
            .map_err(error)
    }
}

fn recenter_after_drag(
    connection: &RustConnection,
    root: u32,
    window: u32,
    area: Rect,
) -> Result<bool, String> {
    let pointer = connection
        .query_pointer(root)
        .map_err(error)?
        .reply()
        .map_err(error)?;
    let buttons = KeyButMask::BUTTON1
        | KeyButMask::BUTTON2
        | KeyButMask::BUTTON3
        | KeyButMask::BUTTON4
        | KeyButMask::BUTTON5;
    // The window manager owns an active resize grab; moving its frame would shift the grabbed edge.
    if pointer.mask.intersects(buttons) {
        return Ok(false);
    }
    if connection
        .get_window_attributes(window)
        .map_err(error)?
        .reply()
        .map_err(error)?
        .map_state
        != MapState::VIEWABLE
    {
        return Ok(true);
    }
    let geometry = connection
        .get_geometry(window)
        .map_err(error)?
        .reply()
        .map_err(error)?;
    let extents = property(
        connection,
        window,
        "_NET_FRAME_EXTENTS",
        AtomEnum::CARDINAL.into(),
    )?;
    let frame_width = extents.first().copied().unwrap_or(0) + extents.get(1).copied().unwrap_or(0);
    let x = area.x + (area.width - geometry.width as i32 - frame_width as i32) / 2;
    connection
        .configure_window(window, &ConfigureWindowAux::new().x(x).y(area.y))
        .map_err(error)?
        .check()
        .map_err(error)?;
    connection.flush().map_err(error)?;
    Ok(true)
}

fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn atom(connection: &RustConnection, name: &str) -> Result<u32, String> {
    Ok(connection
        .intern_atom(false, name.as_bytes())
        .map_err(error)?
        .reply()
        .map_err(error)?
        .atom)
}

fn active_window(connection: &RustConnection, root: u32) -> Result<u32, String> {
    Ok(property(
        connection,
        root,
        "_NET_ACTIVE_WINDOW",
        AtomEnum::WINDOW.into(),
    )?
    .first()
    .copied()
    .unwrap_or(0))
}

fn herdr_window(connection: &RustConnection, mut window: u32, panel: u32) -> Result<bool, String> {
    let mut visited = std::collections::HashSet::new();
    while window != 0 && visited.insert(window) {
        if window == panel
            || property(connection, window, "_NET_WM_PID", AtomEnum::CARDINAL.into())?.first()
                == Some(&std::process::id())
        {
            return Ok(true);
        }
        window = property(
            connection,
            window,
            "WM_TRANSIENT_FOR",
            AtomEnum::WINDOW.into(),
        )?
        .first()
        .copied()
        .unwrap_or(0);
    }
    Ok(false)
}

fn property(
    connection: &RustConnection,
    window: u32,
    name: &str,
    kind: u32,
) -> Result<Vec<u32>, String> {
    let reply = connection
        .get_property(false, window, atom(connection, name)?, kind, 0, 1024)
        .map_err(error)?
        .reply()
        .map_err(error)?;
    Ok(reply
        .value32()
        .map(|values| values.collect())
        .unwrap_or_default())
}

fn keysym(key: &chord::Key) -> u32 {
    use chord::Key;
    match key {
        Key::Character(c) => *c as u32,
        Key::Function(n) => 0xffbe + *n as u32 - 1,
        Key::Backquote => 0x60,
        Key::Space => 0x20,
        Key::Escape => 0xff1b,
        Key::Tab => 0xff09,
        Key::Enter => 0xff0d,
    }
}

fn lock_combinations(ignored: u16) -> Vec<u16> {
    (0..=255).filter(|mask| mask & !ignored == 0).collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl Rect {
    fn intersection(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let width = (self.x + self.width).min(other.x + other.width) - x;
        let height = (self.y + self.height).min(other.y + other.height) - y;
        (width > 0 && height > 0).then_some(Self {
            x,
            y,
            width,
            height,
        })
    }

    fn top(self, width: f32, height: f32) -> Self {
        self.with_size(
            (self.width as f32 * width).round() as i32,
            (self.height as f32 * height).round() as i32,
        )
    }

    fn with_size(self, width: i32, height: i32) -> Self {
        let width = width.clamp(1, self.width.max(1));
        let height = height.clamp(1, self.height.max(1));
        Self {
            x: self.x + (self.width - width) / 2,
            y: self.y,
            width,
            height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_ignore_only_lock_modifiers() {
        assert_eq!(lock_combinations(0), vec![0]);
        assert_eq!(lock_combinations(2 | 16), vec![0, 2, 16, 18]);
        assert_eq!(
            keysym(&chord::parse("ctrl-alt-backquote").unwrap().key),
            0x60
        );
        assert_eq!(keysym(&chord::parse("super-f24").unwrap().key), 0xffd5);
        assert!(validate_shortcut("f12").is_ok());
        assert!(validate_shortcut("right-cmd-space").is_err());
    }

    #[test]
    fn layout_group_aliases_do_not_collapse_overlapping_letter_keys() {
        let names = KeyNames {
            connection: None,
            names: std::collections::HashMap::from([
                ("y".into(), "y".into()),
                ("z".into(), "y".into()),
                ("(".into(), "9".into()),
            ]),
        };
        assert_ne!(names.normalize("y"), names.normalize("z"));
        assert_eq!(names.normalize("("), names.normalize("9"));
    }

    #[test]
    fn panel_respects_monitor_and_desktop_workarea() {
        let monitor = Rect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let desktop = Rect {
            x: -1920,
            y: 32,
            width: 3840,
            height: 1048,
        };
        assert_eq!(
            monitor.intersection(desktop).unwrap().top(0.75, 0.5),
            Rect {
                x: -1680,
                y: 32,
                width: 1440,
                height: 524
            }
        );
        assert_eq!(monitor.intersection(Rect { x: 0, ..monitor }), None);
        assert_eq!(monitor.with_size(2400, 1400), monitor);
    }
}
