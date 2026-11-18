//! A persistent terminal window summoned from other applications.

use gpui::Window;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[cfg(any(not(target_os = "macos"), test))]
mod release;
#[cfg(any(not(target_os = "macos"), test))]
use release::released_status;

static RECORDING_ACTIVE: AtomicBool = AtomicBool::new(false);
static RECORDING: LazyLock<Mutex<(Option<u64>, Value)>> =
    LazyLock::new(|| Mutex::new((None, json!({ "state": "idle" }))));

pub fn validate_shortcut(shortcut: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::validate_shortcut(shortcut).map(|_| ())
    }
    #[cfg(windows)]
    {
        windows::validate_shortcut(shortcut)
    }
    #[cfg(target_os = "linux")]
    {
        linux::validate_shortcut(shortcut)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Permission {
    pub required: bool,
    pub granted: bool,
    pub restart_required: bool,
    pub recheck_available: bool,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Default)]
struct PermissionVisit {
    awaiting_grant: bool,
}

#[cfg(any(target_os = "macos", test))]
#[derive(PartialEq, Eq)]
enum PermissionForeground {
    Settings,
    #[cfg(target_os = "macos")]
    AuthorizationDialog,
    Other,
}

#[cfg(any(target_os = "macos", test))]
impl PermissionVisit {
    fn should_return(
        &mut self,
        relevant: bool,
        granted: bool,
        foreground: impl FnOnce() -> PermissionForeground,
    ) -> bool {
        if !relevant {
            self.awaiting_grant = false;
        }
        if !self.awaiting_grant || !granted {
            return false;
        }
        let front = foreground();
        #[cfg(target_os = "macos")]
        if front == PermissionForeground::AuthorizationDialog {
            return false;
        }
        self.awaiting_grant = false;
        front == PermissionForeground::Settings
    }
}

fn publish_recording(owner: u64, status: Value) {
    *RECORDING.lock().unwrap() = (Some(owner), status);
}

fn prepare_recording(owner: u64) {
    publish_recording(
        owner,
        json!({ "state": "recording", "ready": false, "phase": "idle" }),
    );
}

pub fn recording_status(owner: u64) -> Value {
    let state = RECORDING.lock().unwrap();
    if state.0 == Some(owner) {
        state.1.clone()
    } else {
        json!({ "state": "idle" })
    }
}

#[cfg(windows)]
pub fn recording_active() -> bool {
    RECORDING_ACTIVE.load(Ordering::SeqCst)
}

/// Reserved global keys reach the recorder through their native hotkey callback.
#[cfg(not(target_os = "macos"))]
pub fn record_shortcut(shortcut: &str) -> bool {
    if !RECORDING_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }
    crate::mods::request(crate::mods::Request::ShortcutRecorded {
        shortcut: shortcut.into(),
    });
    true
}

pub fn recording_error(owner: u64, error: impl Into<String>) {
    publish_recording(
        owner,
        json!({ "state": "error", "error": error.into(), "ready": true, "phase": "complete" }),
    );
}

#[cfg(any(not(target_os = "macos"), test))]
fn record_normalized_key(
    previous: Value,
    key: &gpui::Keystroke,
    name: &str,
    shift_down: bool,
) -> Value {
    let modifiers = &key.modifiers;
    if name == "escape"
        && !modifiers.control
        && !modifiers.alt
        && !shift_down
        && !modifiers.platform
    {
        return json!({ "state": "cancelled" });
    }
    if previous["state"] != "recording" {
        return previous;
    }
    let mut parts = Vec::new();
    if modifiers.control {
        parts.push("ctrl");
    }
    if modifiers.alt {
        parts.push("alt");
    }
    if shift_down {
        parts.push("shift");
    }
    if modifiers.platform {
        parts.push("super");
    }
    parts.push(name);
    let shortcut = parts.join("-");
    match validate_shortcut(&shortcut) {
        Ok(()) => {
            json!({ "state": "recorded", "shortcut": shortcut, "ready": false, "phase": "releasing" })
        }
        Err(error) => {
            json!({ "state": "recording", "error": error, "ready": false, "phase": "idle" })
        }
    }
}

fn recording_complete(status: &Value) -> bool {
    status["state"] == "recorded" && status["ready"] == true
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedSize {
    pub width: f32,
    pub height: f32,
}

impl SavedSize {
    fn valid(self) -> bool {
        [self.width, self.height]
            .into_iter()
            .all(|v| v.is_finite() && (1.0..=100_000.0).contains(&v))
    }
}

#[derive(Serialize, Deserialize)]
struct SizePreference {
    platform: String,
    configured_width: f32,
    configured_height: f32,
    size: SavedSize,
}

fn size_path() -> std::path::PathBuf {
    crate::mods::app_dir().join("quick-terminal-window.json")
}

fn saved_size(path: &std::path::Path, config: &Config) -> Option<SavedSize> {
    let text = std::fs::read_to_string(path).ok()?;
    let preference: SizePreference = serde_json::from_str(&text).ok()?;
    if preference.platform != std::env::consts::OS || !preference.size.valid() {
        return None;
    }
    if preference.configured_width != config.width || preference.configured_height != config.height
    {
        if let Err(error) = std::fs::remove_file(path) {
            tracing::warn!("cannot reset quick terminal size: {error}");
        }
        return None;
    }
    Some(preference.size)
}

fn size_text(size: SavedSize, config: &Config) -> String {
    serde_json::to_string_pretty(&SizePreference {
        platform: std::env::consts::OS.into(),
        configured_width: config.width,
        configured_height: config.height,
        size,
    })
    .expect("validated quick terminal size")
        + "\n"
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::Native;
#[cfg(target_os = "macos")]
pub use macos::{keyboard_layout_changed, shortcut_key_label};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_recording;
#[cfg(windows)]
use windows::{KeyNames, Native};
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::{KeyNames, Native};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub shortcut: String,
    /// Fractions of the screen's usable width and height.
    pub width: f32,
    pub height: f32,
    pub auto_hide: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            shortcut: if cfg!(target_os = "macos") {
                "double-cmd"
            } else {
                "ctrl-alt-backquote"
            }
            .into(),
            width: 1.0,
            height: 0.6,
            auto_hide: true,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [("width", self.width), ("height", self.height)] {
            if !value.is_finite() || !(0.1..=1.0).contains(&value) {
                return Err(format!("quick_terminal.{name} must be between 0.1 and 1"));
            }
        }
        if self.shortcut.trim().is_empty() {
            return Err("quick_terminal.shortcut must not be empty".into());
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct QuickTerminal {
    #[cfg(target_os = "macos")]
    permission_visit: PermissionVisit,
    native: Option<Native>,
    error: Option<String>,
    config: Config,
    saved_size: Option<SavedSize>,
    record_owner: Option<u64>,
    record_focus: Option<gpui::FocusHandle>,
    #[cfg(target_os = "macos")]
    recorder: Option<macos::Recorder>,
    #[cfg(not(target_os = "macos"))]
    recorder: Option<gpui::Subscription>,
    #[cfg(windows)]
    modifier_recorder: Option<windows_recording::Recorder>,
    #[cfg(not(target_os = "macos"))]
    record_drain: std::rc::Rc<std::cell::RefCell<release::Drain>>,
    #[cfg(not(target_os = "macos"))]
    record_key_names: std::rc::Rc<KeyNames>,
}

impl QuickTerminal {
    pub fn open_permission_settings(&mut self, saved: &Config) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let granted = macos::accessibility_trusted();
            macos::open_permission_settings(self.native.as_ref())?;
            self.permission_visit.awaiting_grant =
                !granted && self.permission_visit_relevant(saved);
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = saved;
            Err("Quick-terminal Accessibility permission is only needed on macOS".into())
        }
    }

    #[cfg(target_os = "macos")]
    fn permission_visit_relevant(&self, saved: &Config) -> bool {
        macos::shortcut_needs_accessibility(&saved.shortcut)
            || self
                .native
                .as_ref()
                .is_some_and(Native::uses_double_modifier)
    }

    pub fn should_return_from_permission_settings(
        &mut self,
        permission: &Permission,
        saved: &Config,
    ) -> bool {
        #[cfg(target_os = "macos")]
        {
            if !self.permission_visit.awaiting_grant {
                return false;
            }
            let relevant = self.permission_visit_relevant(saved);
            self.permission_visit.should_return(
                relevant,
                permission.granted,
                macos::permission_foreground,
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (permission, saved);
            false
        }
    }

    pub fn start(window: &Window, config: &Config) -> Result<Self, String> {
        config.validate()?;
        let mut native = Native::new(window, config)?;
        let saved_size = saved_size(&size_path(), config);
        native.set_saved_size(saved_size);
        native.show()?;
        Ok(Self {
            config: config.clone(),
            native: Some(native),
            saved_size,
            ..Default::default()
        })
    }

    pub fn inactive(config: &Config, error: Option<String>) -> Self {
        Self {
            config: config.clone(),
            error,
            ..Default::default()
        }
    }

    pub fn restart_required(&self, saved: &Config) -> bool {
        self.config.enabled != saved.enabled
            || self.config.shortcut != saved.shortcut
            || self.config.width != saved.width
            || self.config.height != saved.height
    }

    pub fn enabled(&self) -> bool {
        self.native.is_some()
    }

    pub fn auto_hide(&self) -> bool {
        self.config.auto_hide
    }

    pub fn set_auto_hide(&mut self, enabled: bool) {
        if self.config.auto_hide == enabled {
            return;
        }
        if let Some(native) = &mut self.native {
            native.set_auto_hide(enabled);
        }
        self.config.auto_hide = enabled;
    }

    pub fn hide_on_blur(&mut self) -> Result<(), String> {
        if self.config.auto_hide
            && let Some(native) = &mut self.native
        {
            native.hide()?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn set_shortcut_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn warning(&self, permission: &Permission) -> Option<&str> {
        #[cfg(target_os = "macos")]
        {
            self.native
                .as_ref()
                .and_then(|native| native.warning(permission.granted))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = permission;
            None
        }
    }

    pub fn permission(&mut self, saved: &Config) -> Permission {
        #[cfg(target_os = "macos")]
        {
            let granted = macos::accessibility_trusted();
            if let Some(native) = &mut self.native {
                native.observe_permission(granted);
            }
            self.permission_state(saved, granted)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = saved;
            Permission {
                required: false,
                granted: true,
                restart_required: false,
                recheck_available: false,
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn permission_state(&self, saved: &Config, granted: bool) -> Permission {
        let selected = macos::shortcut_needs_accessibility(&saved.shortcut);
        let active = self
            .native
            .as_ref()
            .is_some_and(Native::uses_double_modifier);
        Permission {
            required: active || (saved.enabled && selected),
            granted,
            recheck_available: self.permission_visit.awaiting_grant
                && !granted
                && (active || selected),
            restart_required: granted
                && self
                    .native
                    .as_ref()
                    .is_some_and(Native::permission_restart_required),
        }
    }

    pub fn should_poll_permission(&self, saved: &Config, settings_open: bool) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.permission_visit.awaiting_grant
                || self
                    .native
                    .as_ref()
                    .is_some_and(Native::uses_double_modifier)
                || ((saved.enabled || settings_open)
                    && macos::shortcut_needs_accessibility(&saved.shortcut))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (saved, settings_open);
            false
        }
    }

    pub fn diagnostics(&self) -> serde_json::Value {
        #[cfg(target_os = "macos")]
        {
            self.native
                .as_ref()
                .map(Native::diagnostics)
                .unwrap_or(serde_json::Value::Null)
        }
        #[cfg(not(target_os = "macos"))]
        {
            serde_json::Value::Null
        }
    }

    pub fn visible(&self) -> bool {
        self.native.as_ref().is_some_and(Native::is_visible)
    }

    pub fn resized(&mut self, cx: &gpui::App) -> Option<gpui::Task<()>> {
        let size = self.native.as_mut()?.resized()?;
        if !size.valid() || self.saved_size == Some(size) {
            return None;
        }
        self.saved_size = Some(size);
        let text = size_text(size, &self.config);
        let path = size_path();
        let executor = cx.background_executor().clone();
        Some(cx.background_executor().spawn(async move {
            executor.timer(std::time::Duration::from_millis(400)).await;
            if let Err(error) = crate::persist::write_atomic(&path, text) {
                tracing::warn!("cannot save quick terminal size: {error}");
            }
        }))
    }

    pub fn save_size(&self) {
        if let Some(size) = self.saved_size
            && let Err(error) =
                crate::persist::write_atomic(&size_path(), size_text(size, &self.config))
        {
            tracing::warn!("cannot save quick terminal size: {error}");
        }
    }

    pub fn record_owner(&self) -> Option<u64> {
        self.record_owner
    }

    pub fn start_recording(
        &mut self,
        owner: u64,
        window: &Window,
        cx: &mut gpui::App,
    ) -> Result<(), String> {
        self.cancel_recording();
        #[cfg(target_os = "macos")]
        {
            if let Some(native) = &mut self.native {
                native.set_shortcut_suspended(true)?;
            }
            match macos::Recorder::start(window, owner) {
                Ok(recorder) => self.recorder = Some(recorder),
                Err(error) => {
                    if let Err(restore) = self.restore_shortcut() {
                        let combined = format!(
                            "{error}. The original shortcut could not be restored: {restore}"
                        );
                        self.error = Some(combined.clone());
                        return Err(combined);
                    }
                    return Err(error);
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let focus = window
                .focused(cx)
                .ok_or("Focus the shortcut field before recording")?;
            let drain = std::rc::Rc::new(std::cell::RefCell::new(release::Drain::default()));
            let key_names = std::rc::Rc::new(KeyNames::new()?);
            #[cfg(windows)]
            {
                self.modifier_recorder = Some(windows_recording::Recorder::start(window)?);
            }
            self.record_drain = drain.clone();
            self.record_key_names = key_names.clone();
            self.recorder = Some(cx.intercept_keystrokes(move |event, window, cx| {
                if !focus.is_focused(window) || !window.is_window_active() {
                    return;
                }
                cx.stop_propagation();
                let mut drain = drain.borrow_mut();
                let key = &event.keystroke;
                let name = key_names.normalize(&key.key);
                if !drain.press(&name) {
                    return;
                }
                // GPUI folds Shift into symbols such as ! on a number key.
                let shift = key.modifiers.shift || window.modifiers().shift;
                let status = record_normalized_key(recording_status(owner), key, &name, shift);
                publish_recording(
                    owner,
                    released_status(
                        status,
                        &drain,
                        window.modifiers().number_of_modifiers() != 0,
                    ),
                );
            }));
        }
        self.record_owner = Some(owner);
        self.record_focus = window.focused(cx);
        RECORDING_ACTIVE.store(true, Ordering::SeqCst);
        prepare_recording(owner);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn recorded_shortcut(&mut self, shortcut: &str) {
        let Some(owner) = self.record_owner else {
            return;
        };
        {
            let mut drain = self.record_drain.borrow_mut();
            drain.native_source = true;
            drain.native_pending = true;
        }
        let previous = recording_status(owner);
        if previous["state"] == "recording" {
            match validate_shortcut(shortcut) {
                Ok(()) => publish_recording(
                    owner,
                    json!({ "state": "recorded", "shortcut": shortcut, "ready": false, "phase": "releasing" }),
                ),
                Err(error) => recording_error(owner, error),
            }
        } else {
            publish_recording(
                owner,
                released_status(previous, &self.record_drain.borrow(), true),
            );
        }
    }

    pub fn recording_key_up(&mut self, event: &gpui::KeyUpEvent, window: &Window) -> bool {
        #[cfg(not(target_os = "macos"))]
        {
            if self.record_owner.is_none() {
                return false;
            }
            self.record_drain
                .borrow_mut()
                .release(&self.record_key_names.normalize(&event.keystroke.key));
            self.poll_recording(window);
            true
        }
        #[cfg(target_os = "macos")]
        {
            let _ = (event, window);
            false
        }
    }

    pub fn poll_recording(&mut self, window: &Window) {
        let Some(owner) = self.record_owner else {
            return;
        };
        #[cfg(target_os = "macos")]
        if let Some(recorder) = &self.recorder {
            publish_recording(owner, recorder.status());
        }
        if !window.is_window_active()
            || self
                .record_focus
                .as_ref()
                .is_some_and(|focus| !focus.is_focused(window))
        {
            self.stop_recording(recording_complete(&recording_status(owner)));
            return;
        }
        #[cfg(not(target_os = "macos"))]
        {
            match self.native.as_ref().map_or_else(
                || self.record_key_names.keyboard_released(),
                Native::keyboard_released,
            ) {
                Ok(released) => self.record_drain.borrow_mut().native_pending = !released,
                Err(error) => {
                    recording_error(owner, error);
                    self.stop_recording(true);
                    return;
                }
            }
            let status = recording_status(owner);
            #[cfg(windows)]
            let status = if let Some(recorder) = &self.modifier_recorder {
                recorder.status(status)
            } else {
                status
            };
            publish_recording(
                owner,
                released_status(
                    status,
                    &self.record_drain.borrow(),
                    window.modifiers().number_of_modifiers() != 0,
                ),
            );
        }
        if recording_status(owner)["state"] == "cancelled" {
            self.cancel_recording();
        }
    }

    pub fn cancel_recording(&mut self) {
        self.stop_recording(false);
    }

    #[cfg(target_os = "macos")]
    fn restore_shortcut(&mut self) -> Result<(), String> {
        let Some(native) = &mut self.native else {
            return Ok(());
        };
        let result = native.set_shortcut_suspended(false);
        self.error = result.as_ref().err().cloned();
        result
    }

    fn stop_recording(&mut self, preserve_status: bool) {
        let owner = self.record_owner.take();
        #[cfg(target_os = "macos")]
        if let Some(mut recorder) = self.recorder.take()
            && !preserve_status
        {
            recorder.cancel();
        }
        #[cfg(not(target_os = "macos"))]
        self.recorder.take();
        #[cfg(windows)]
        self.modifier_recorder.take();
        self.record_focus = None;
        RECORDING_ACTIVE.store(false, Ordering::SeqCst);
        if let Some(owner) = owner {
            if !preserve_status {
                publish_recording(owner, json!({ "state": "cancelled" }));
            }
            #[cfg(target_os = "macos")]
            if let Err(error) = self.restore_shortcut() {
                recording_error(owner, error.clone());
                crate::mods::request(crate::mods::Request::OpenOverlay {
                    id: crate::mods::new_overlay_id(),
                    spec: json!({ "kind": "toast", "notification": { "title": "Shortcut could not be restored", "body": error, "level": "error" } }),
                });
            }
        }
    }

    pub fn run(&mut self, command: &str) -> Result<(), String> {
        let unavailable = self.error.clone().unwrap_or_else(|| {
            "Enable quick_terminal.enabled in settings.json and restart Herdr first".into()
        });
        let native = self.native.as_mut().ok_or(unavailable)?;
        match command {
            "quick_terminal.show" => native.show(),
            "quick_terminal.hide" => native.hide(),
            "quick_terminal.toggle" => {
                if native.is_visible() {
                    native.hide()
                } else {
                    native.show()
                }
            }
            _ => Err(format!("unknown quick terminal command: {command}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded_key(previous: Value, key: &gpui::Keystroke) -> Value {
        record_normalized_key(previous, key, &key.key, key.modifiers.shift)
    }

    #[test]
    fn recorded_shifted_number_keeps_the_physical_modifier() {
        let symbol = gpui::Keystroke::parse("ctrl-!").unwrap();
        let status = record_normalized_key(json!({ "state": "recording" }), &symbol, "1", true);
        assert_eq!(status["shortcut"], "ctrl-shift-1");
        let plain = record_normalized_key(json!({ "state": "recording" }), &symbol, "1", false);
        assert_eq!(plain["shortcut"], "ctrl-1");
    }

    #[test]
    fn permission_visit_returns_once_after_a_confirmed_grant() {
        let mut visit = PermissionVisit::default();
        assert!(!visit.should_return(true, true, || unreachable!()));
        visit.awaiting_grant = true;
        assert!(!visit.should_return(true, false, || unreachable!()));
        #[cfg(target_os = "macos")]
        assert!(!visit.should_return(true, true, || PermissionForeground::AuthorizationDialog));
        assert!(visit.should_return(true, true, || PermissionForeground::Settings));
        assert!(!visit.should_return(true, true, || unreachable!()));
        visit.awaiting_grant = true;
        assert!(!visit.should_return(true, true, || PermissionForeground::Other));
        assert!(!visit.should_return(true, true, || unreachable!()));
        visit.awaiting_grant = true;
        assert!(!visit.should_return(false, false, || unreachable!()));
        assert!(!visit.should_return(true, true, || unreachable!()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn permission_polling_waits_for_an_enabled_shortcut_or_open_settings() {
        let terminal = QuickTerminal::default();
        let mut saved = Config::default();
        assert!(!terminal.should_poll_permission(&saved, false));
        assert!(terminal.should_poll_permission(&saved, true));
        saved.enabled = true;
        assert!(terminal.should_poll_permission(&saved, false));
        saved.shortcut = "ctrl-alt-space".into();
        assert!(!terminal.should_poll_permission(&saved, true));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn permission_visit_supports_the_disabled_default_and_cancels_for_a_chord() {
        let mut terminal = QuickTerminal::default();
        let saved = Config::default();
        assert!(terminal.permission_visit_relevant(&saved));
        assert!(!terminal.permission_state(&saved, false).recheck_available);
        terminal.permission_visit.awaiting_grant = true;
        let missing = terminal.permission_state(&saved, false);
        assert!(!missing.required);
        assert!(missing.recheck_available);
        assert!(!terminal.should_return_from_permission_settings(&missing, &saved));
        let granted = terminal.permission_state(&saved, true);
        assert!(!granted.restart_required);
        assert!(!granted.recheck_available);
        assert!(
            terminal
                .permission_state(
                    &Config {
                        enabled: true,
                        ..saved.clone()
                    },
                    true
                )
                .required
        );

        let chord = Config {
            shortcut: "ctrl-alt-space".into(),
            ..saved
        };
        let missing = terminal.permission_state(&chord, false);
        assert!(!missing.required);
        assert!(!missing.recheck_available);
        assert!(!terminal.should_return_from_permission_settings(&missing, &chord));
        assert!(!terminal.permission_visit.awaiting_grant);
    }

    #[test]
    fn escape_cancels_recording_and_completed_preview() {
        let escape = gpui::Keystroke::parse("escape").unwrap();
        for previous in [
            json!({"state":"recording"}),
            json!({"state":"recorded", "shortcut":"ctrl-q"}),
        ] {
            assert_eq!(recorded_key(previous, &escape)["state"], "cancelled");
        }
    }

    #[test]
    fn recording_requires_a_safe_key_and_keeps_the_completed_preview() {
        assert_eq!(
            recorded_key(
                json!({"state":"recording"}),
                &gpui::Keystroke::parse("a").unwrap()
            )["state"],
            "recording"
        );
        let recorded = recorded_key(
            json!({"state":"recording"}),
            &gpui::Keystroke::parse("ctrl-alt-space").unwrap(),
        );
        assert_eq!(recorded["shortcut"], "ctrl-alt-space");
        assert_eq!(
            recorded_key(recorded.clone(), &gpui::Keystroke::parse("ctrl-q").unwrap()),
            recorded
        );
    }

    #[test]
    fn recorded_shortcut_waits_for_keys_and_modifiers_and_revokes_readiness_on_overlap() {
        let mut drain = release::Drain::default();
        drain.press("space");
        let status = recorded_key(
            json!({ "state": "recording" }),
            &gpui::Keystroke::parse("ctrl-alt-space").unwrap(),
        );
        assert!(!recording_complete(&status));
        let status = released_status(status, &drain, false);
        assert_eq!(status["phase"], "releasing");
        drain.release("space");
        assert!(!recording_complete(&released_status(
            status.clone(),
            &drain,
            true
        )));
        let status = released_status(status, &drain, false);
        assert!(recording_complete(&status));
        assert_eq!(status["phase"], "complete");
        drain.press("q");
        assert!(!recording_complete(&released_status(status, &drain, false)));
        assert!(!recording_complete(
            &json!({ "state": "recorded", "shortcut": "f8" })
        ));
    }

    #[test]
    fn old_settings_leave_quick_terminal_off() {
        let person: crate::settings::Person = serde_json::from_str("{}").unwrap();
        assert!(!person.quick_terminal.enabled);
        person.quick_terminal.validate().unwrap();
    }

    #[test]
    fn invalid_dimensions_are_rejected() {
        for value in [0.0, -1.0, 1.01, f32::NAN, f32::INFINITY] {
            assert!(
                Config {
                    width: value,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
            assert!(
                Config {
                    height: value,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(serde_json::from_str::<Config>(r#"{"auto_hdie":true}"#).is_err());
    }

    #[test]
    fn disabled_commands_report_an_actionable_error() {
        let mut terminal = QuickTerminal::default();
        assert!(!terminal.enabled());
        assert!(!terminal.visible());
        assert!(
            terminal
                .run("quick_terminal.toggle")
                .unwrap_err()
                .contains("restart")
        );
    }

    #[test]
    fn live_auto_hide_keeps_restart_bound_settings_and_manual_size() {
        let config = Config {
            enabled: true,
            shortcut: "ctrl-alt-space".into(),
            width: 0.8,
            height: 0.5,
            auto_hide: true,
        };
        let size = SavedSize {
            width: 900.0,
            height: 500.0,
        };
        let mut terminal = QuickTerminal {
            config: config.clone(),
            saved_size: Some(size),
            ..Default::default()
        };
        terminal.set_auto_hide(false);
        assert!(!terminal.auto_hide());
        assert_eq!(
            terminal.config,
            Config {
                auto_hide: false,
                ..config.clone()
            }
        );
        assert_eq!(terminal.saved_size, Some(size));
        terminal.hide_on_blur().unwrap();
        terminal.set_auto_hide(true);
        assert_eq!(terminal.config, config);
        assert_eq!(terminal.saved_size, Some(size));
    }

    #[test]
    fn restart_tracks_each_startup_setting_and_clears_after_reverting() {
        let startup = Config {
            enabled: true,
            shortcut: "ctrl-alt-space".into(),
            width: 0.8,
            height: 0.5,
            auto_hide: true,
        };
        let mut terminal = QuickTerminal::inactive(&startup, None);
        assert!(!terminal.restart_required(&startup));
        let changes = [
            Config {
                enabled: false,
                ..startup.clone()
            },
            Config {
                shortcut: "ctrl-alt-backquote".into(),
                ..startup.clone()
            },
            Config {
                width: 0.9,
                ..startup.clone()
            },
            Config {
                height: 0.6,
                ..startup.clone()
            },
        ];
        for saved in changes {
            assert!(terminal.restart_required(&saved));
            assert!(
                !terminal.restart_required(&startup),
                "reverting must clear the pending restart"
            );
        }
        let live_change = Config {
            auto_hide: false,
            ..startup.clone()
        };
        assert!(!terminal.restart_required(&live_change));
        terminal.set_auto_hide(false);
        assert!(!terminal.restart_required(&live_change));
        assert!(!terminal.restart_required(&startup));
    }

    #[test]
    fn disabled_and_failed_startup_keep_the_configured_baseline() {
        let config = Config {
            enabled: false,
            shortcut: "ctrl-alt-space".into(),
            width: 0.7,
            height: 0.4,
            auto_hide: false,
        };
        let disabled = QuickTerminal::inactive(&config, None);
        assert_eq!(disabled.config, config);
        assert!(!disabled.restart_required(&config));
        let enabled = Config {
            enabled: true,
            ..config
        };
        assert!(disabled.restart_required(&enabled));
        let failed =
            QuickTerminal::inactive(&enabled, Some("shortcut is already registered".into()));
        assert!(!failed.enabled());
        assert_eq!(failed.error(), Some("shortcut is already registered"));
        assert_eq!(failed.config, enabled);
        assert!(!failed.restart_required(&enabled));
        assert!(failed.restart_required(&Config::default()));
    }

    #[test]
    fn restart_compares_normalized_defaults_and_f32_dimensions() {
        let startup: Config = serde_json::from_str(r#"{"width":0.8,"height":0.6}"#).unwrap();
        let terminal = QuickTerminal::inactive(&startup, None);
        let saved: Config =
            serde_json::from_str(r#"{"width":0.80000000000000004,"height":0.6000000000000001}"#)
                .unwrap();
        assert!(!terminal.restart_required(&saved));
        assert!(terminal.restart_required(&Config {
            width: 0.81,
            ..saved
        }));
        let defaults = QuickTerminal::inactive(&Config::default(), None);
        assert!(!defaults.restart_required(&serde_json::from_str("{}").unwrap()));
    }

    fn temporary_size_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "herdr-quick-size-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn manual_size_survives_restart_until_configured_dimensions_change() {
        let path = temporary_size_path();
        let config = Config::default();
        let size = SavedSize {
            width: 900.0,
            height: 500.0,
        };
        let text = size_text(size, &config);
        for changed in [
            Config {
                width: 0.8,
                ..config.clone()
            },
            Config {
                height: 0.8,
                ..config.clone()
            },
        ] {
            std::fs::write(&path, &text).unwrap();
            assert_eq!(saved_size(&path, &config), Some(size));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
            assert_eq!(saved_size(&path, &changed), None);
            assert!(
                !path.exists(),
                "changed dimensions discard the previous manual size"
            );
            assert_eq!(
                saved_size(&path, &config),
                None,
                "restoring settings must not resurrect the old manual size"
            );
        }
    }

    #[test]
    fn invalid_and_foreign_size_preferences_are_left_untouched() {
        let path = temporary_size_path();
        let config = Config::default();
        let text = size_text(
            SavedSize {
                width: 900.0,
                height: 500.0,
            },
            &config,
        );
        for ignored in [
            text.replace("900.0", "-1.0"),
            "invalid json".into(),
            text.replace(std::env::consts::OS, "another-platform"),
        ] {
            std::fs::write(&path, &ignored).unwrap();
            assert_eq!(saved_size(&path, &config), None);
            assert_eq!(
                saved_size(
                    &path,
                    &Config {
                        width: 0.8,
                        ..config.clone()
                    }
                ),
                None
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), ignored);
        }
        std::fs::remove_file(path).unwrap();
    }
}
