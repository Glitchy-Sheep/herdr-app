//! Background input for an explicitly configured double-modifier shortcut.

use super::double_tap::{DoubleTap, Gesture, MAX_GAP_MS, ModifierKey, TapPhase};
use std::{io, ptr, sync::Mutex};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::{
        Input::{
            GetRawInputData, GetRegisteredRawInputDevices,
            KeyboardAndMouse::{
                GetAsyncKeyState, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU,
                VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_SHIFT,
            },
            RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RAWKEYBOARD, RID_INPUT, RIDEV_INPUTSINK,
            RIDEV_REMOVE, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE, RegisterRawInputDevices,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetQueueStatus, HWND_MESSAGE,
            KillTimer, MSG, QS_RAWINPUT, RI_KEY_BREAK, RI_KEY_E0, RI_KEY_E1, RIM_INPUT, SetTimer,
            WM_INPUT, WM_TIMER,
        },
    },
};

const MOUSE: u16 = 2;
const KEYBOARD: u16 = 6;
const GESTURE_TIMER: usize = 1;
static REGISTRATION: Mutex<()> = Mutex::new(());

struct MessageWindow(HWND);

impl MessageWindow {
    fn new() -> Result<Self, String> {
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                windows_sys::core::w!("STATIC"),
                ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            Err(format!(
                "Cannot create the double-tap shortcut window: {}",
                io::Error::last_os_error()
            ))
        } else {
            Ok(Self(hwnd))
        }
    }
}

impl Drop for MessageWindow {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) };
    }
}

/// Raw Input registrations are shared by all windows in a process.
struct Device(RAWINPUTDEVICE);

impl Device {
    fn owner(usage: u16) -> io::Result<Option<HWND>> {
        let size = size_of::<RAWINPUTDEVICE>() as u32;
        let mut count = 0;
        if unsafe { GetRegisteredRawInputDevices(ptr::null_mut(), &mut count, size) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            return Ok(None);
        }
        let mut devices = vec![RAWINPUTDEVICE::default(); count as usize];
        let n = unsafe { GetRegisteredRawInputDevices(devices.as_mut_ptr(), &mut count, size) };
        if n == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(devices[..n as usize]
            .iter()
            .find(|d| d.usUsagePage == 1 && d.usUsage == usage)
            .map(|d| d.hwndTarget))
    }

    fn register(hwnd: HWND, usage: u16) -> Result<Self, String> {
        let _lock = REGISTRATION.lock().unwrap();
        if Self::owner(usage).map_err(|e| e.to_string())?.is_some() {
            return Err("Another Herdr window already uses this raw input device".into());
        }
        let device = RAWINPUTDEVICE {
            usUsagePage: 1,
            usUsage: usage,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        if unsafe { RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as u32) } == 0 {
            return Err(format!(
                "Cannot listen for the double-tap shortcut: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(Self(device))
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _lock = REGISTRATION.lock().unwrap();
        if Self::owner(self.0.usUsage).is_ok_and(|owner| owner == Some(self.0.hwndTarget)) {
            let device = RAWINPUTDEVICE {
                dwFlags: RIDEV_REMOVE,
                hwndTarget: ptr::null_mut(),
                ..self.0
            };
            unsafe { RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as u32) };
        }
    }
}

pub(super) struct RawShortcut {
    gesture: Gesture,
    input: GestureInput,
    mouse: Option<Device>,
    _keyboard: Device,
    window: MessageWindow,
    error_reported: bool,
}

impl RawShortcut {
    pub(super) fn new(gesture: Gesture) -> Result<Self, String> {
        let window = MessageWindow::new()?;
        let input = GestureInput::at_registration();
        let keyboard = Device::register(window.0, KEYBOARD)?;
        Ok(Self {
            gesture,
            input,
            mouse: None,
            _keyboard: keyboard,
            window,
            error_reported: false,
        })
    }

    pub(super) fn handle(&mut self, message: &MSG) {
        if message.hwnd != self.window.0 {
            return;
        }
        let completed = match message.message {
            WM_INPUT => {
                let mut packet = RAWINPUT::default();
                let mut size = size_of::<RAWINPUT>() as u32;
                let read = unsafe {
                    GetRawInputData(
                        message.lParam as _,
                        RID_INPUT,
                        (&mut packet as *mut RAWINPUT).cast(),
                        &mut size,
                        size_of::<RAWINPUTHEADER>() as u32,
                    )
                };
                if message.wParam == RIM_INPUT as usize {
                    unsafe {
                        DefWindowProcW(message.hwnd, WM_INPUT, message.wParam, message.lParam)
                    };
                }
                if read == u32::MAX {
                    self.input.tap = DoubleTap::default();
                    false
                } else {
                    self.input.recognize(
                        self.gesture,
                        &packet,
                        message.time,
                        crate::quick_terminal::recording_active(),
                        chord_at_queue_tail,
                    )
                }
            }
            WM_TIMER if message.wParam == GESTURE_TIMER => {
                self.input.tap.refresh(message.time);
                false
            }
            _ => return,
        };
        if let Err(error) = self.watch_buttons() {
            self.input.tap.interrupt();
            if !self.error_reported {
                crate::mods::request(crate::mods::Request::QuickTerminalShortcutError(Some(
                    error.clone(),
                )));
                super::report_error(error);
                self.error_reported = true;
            }
        } else {
            if self.error_reported && self.mouse.is_some() {
                crate::mods::request(crate::mods::Request::QuickTerminalShortcutError(None));
                self.error_reported = false;
            }
            if completed && !crate::quick_terminal::recording_active() {
                super::request("quick_terminal.toggle");
            }
        }
    }

    // Mouse motion should only reach this window while a candidate gesture is in progress.
    fn watch_buttons(&mut self) -> Result<(), String> {
        match (self.input.tap.phase(), self.mouse.is_some()) {
            (TapPhase::Idle, true) => {
                unsafe { KillTimer(self.window.0, GESTURE_TIMER) };
                self.mouse = None;
            }
            (TapPhase::Down(_) | TapPhase::Waiting(_), false) => {
                let mouse = Device::register(self.window.0, MOUSE)?;
                if unsafe { SetTimer(self.window.0, GESTURE_TIMER, MAX_GAP_MS + 1, None) } == 0 {
                    return Err(format!(
                        "Cannot time the double-tap shortcut: {}",
                        io::Error::last_os_error()
                    ));
                }
                self.mouse = Some(mouse);
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Default)]
struct GestureInput {
    tap: DoubleTap,
    pressed: [u64; 4],
}

impl GestureInput {
    fn at_registration() -> Self {
        let mut input = Self::default();
        // A key can already be held when the raw keyboard starts delivering events.
        for vk in 8..=0xfe {
            if unsafe { GetAsyncKeyState(i32::from(vk)) < 0 } {
                input.set_pressed(vk, true);
            }
        }
        input
    }

    fn set_pressed(&mut self, vk: u16, down: bool) {
        if vk > 0xfe || matches!(vk, VK_CONTROL | VK_MENU | VK_SHIFT) {
            return;
        }
        let word = &mut self.pressed[usize::from(vk / 64)];
        let bit = 1u64 << (vk % 64);
        if down {
            *word |= bit;
        } else {
            *word &= !bit;
        }
    }

    fn in_chord(&mut self, key: ModifierKey, live: Option<bool>) -> bool {
        if let Some(chord) = live {
            // At the queue tail, a lone modifier also clears any missed breaks.
            if !chord {
                self.pressed = [0; 4];
                self.set_pressed(key as u16, true);
            }
            return chord;
        }
        self.pressed
            .iter()
            .map(|word| word.count_ones())
            .sum::<u32>()
            > 1
    }

    fn recognize(
        &mut self,
        gesture: Gesture,
        packet: &RAWINPUT,
        time: u32,
        suspended: bool,
        live_chord: impl FnOnce(ModifierKey) -> Option<bool>,
    ) -> bool {
        if suspended {
            self.tap = DoubleTap::default();
        }
        match packet.header.dwType {
            RIM_TYPEKEYBOARD => {
                let event = unsafe { packet.data.keyboard };
                let modifier = physical_modifier(&event);
                let down = u32::from(event.Flags) & RI_KEY_BREAK == 0;
                let vk = modifier.map_or(event.VKey, |key| key as u16);
                self.set_pressed(vk, down);
                if !suspended && let Some(key) = modifier.filter(|key| gesture.accepts(*key)) {
                    let chord =
                        down && self.tap.can_start_press() && self.in_chord(key, live_chord(key));
                    let completed = self.tap.key(key, down, time);
                    if chord {
                        self.tap.interrupt();
                    }
                    return completed;
                }
                self.tap.interrupt();
            }
            RIM_TYPEMOUSE
                if unsafe { packet.data.mouse.Anonymous.Anonymous.usButtonFlags } != 0 =>
            {
                self.tap.interrupt()
            }
            _ => {}
        }
        false
    }
}

fn physical_modifier(event: &RAWKEYBOARD) -> Option<ModifierKey> {
    // Pause and Print Screen can contain prefixes resembling Ctrl or Shift.
    if u32::from(event.Flags) & RI_KEY_E1 != 0 || event.MakeCode == 0xff {
        return None;
    }
    let extended = u32::from(event.Flags) & RI_KEY_E0 != 0;
    let vk = match event.VKey {
        VK_MENU => {
            if extended {
                VK_RMENU
            } else {
                VK_LMENU
            }
        }
        VK_CONTROL => {
            if extended {
                VK_RCONTROL
            } else {
                VK_LCONTROL
            }
        }
        VK_SHIFT if !extended => match event.MakeCode {
            0x2a => VK_LSHIFT,
            0x36 => VK_RSHIFT,
            _ => return None,
        },
        vk => vk,
    };
    ModifierKey::from_vk(vk)
}

// Live keyboard state can include presses and releases from later queued packets.
fn chord_at_queue_tail(modifier: ModifierKey) -> Option<bool> {
    let caught_up = || unsafe { GetQueueStatus(QS_RAWINPUT) } >> 16 == 0;
    if !caught_up() {
        return None;
    }
    let chord = (1..=0xfeu16)
        .filter(|key| *key != modifier as u16 && !matches!(*key, VK_MENU | VK_CONTROL | VK_SHIFT))
        .any(|key| unsafe { GetAsyncKeyState(i32::from(key)) < 0 });
    caught_up().then_some(chord)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{VK_LWIN, VK_RWIN, VK_TAB},
        Input::{RAWINPUT_0, RAWMOUSE, RAWMOUSE_0, RAWMOUSE_0_0},
        WindowsAndMessaging::RI_MOUSE_LEFT_BUTTON_DOWN,
    };

    fn key(vk: u16, scan: u16, flags: u32) -> RAWINPUT {
        RAWINPUT {
            header: RAWINPUTHEADER {
                dwType: RIM_TYPEKEYBOARD,
                ..Default::default()
            },
            data: RAWINPUT_0 {
                keyboard: RAWKEYBOARD {
                    VKey: vk,
                    MakeCode: scan,
                    Flags: flags as u16,
                    ..Default::default()
                },
            },
        }
    }

    #[test]
    fn win32_make_break_packets_preserve_sides_and_fire_on_second_release() {
        for (vk, scan, ext, selected) in [
            (VK_MENU, 0x38, 0, ModifierKey::LeftAlt),
            (VK_MENU, 0x38, RI_KEY_E0, ModifierKey::RightAlt),
            (VK_CONTROL, 0x1d, 0, ModifierKey::LeftCtrl),
            (VK_CONTROL, 0x1d, RI_KEY_E0, ModifierKey::RightCtrl),
            (VK_SHIFT, 0x2a, 0, ModifierKey::LeftShift),
            (VK_SHIFT, 0x36, 0, ModifierKey::RightShift),
            (VK_LWIN, 0x5b, RI_KEY_E0, ModifierKey::LeftWin),
            (VK_RWIN, 0x5c, RI_KEY_E0, ModifierKey::RightWin),
        ] {
            let mut input = GestureInput::default();
            let gesture = Gesture::parse(selected.shortcut()).unwrap().unwrap();
            let make = key(vk, scan, ext);
            let release = key(vk, scan, ext | RI_KEY_BREAK);
            for (at, packet, expected) in [
                (10, make, false),
                (40, make, false),
                (60, release, false),
                (105, make, false),
                (130, release, true),
            ] {
                assert_eq!(
                    input.recognize(gesture, &packet, at, false, |_| None),
                    expected
                );
            }
        }
    }

    #[test]
    fn queued_key_edges_keep_a_held_chord_until_its_break() {
        let left = key(VK_MENU, 0x38, 0);
        let up = key(VK_MENU, 0x38, RI_KEY_BREAK);
        let a = u16::from(b'A');
        let gesture = Gesture::parse("double-left-alt").unwrap().unwrap();
        let mut input = GestureInput::default();
        for (at, packet, expected) in [
            (0, key(a, 0x1e, 0), false),
            (5, key(a, 0x1e, 0), false),
            (10, left, false),
            (20, up, false),
            (30, left, false),
            (40, up, false),
            (45, key(a, 0x1e, RI_KEY_BREAK), false),
            (50, left, false),
            (60, up, false),
            (70, left, false),
            (80, up, true),
        ] {
            assert_eq!(
                input.recognize(gesture, &packet, at, false, |_| None),
                expected
            );
        }
    }

    #[test]
    fn a_lone_modifier_at_the_queue_tail_recovers_after_a_missing_break() {
        let left = key(VK_MENU, 0x38, 0);
        let up = key(VK_MENU, 0x38, RI_KEY_BREAK);
        let gesture = Gesture::parse("double-left-alt").unwrap().unwrap();
        let mut input = GestureInput::default();
        for (at, packet, live_chord, expected) in [
            (0, key(u16::from(b'A'), 0x1e, 0), None, false),
            (10, left, Some(false), false),
            (20, up, None, false),
            (30, left, None, false),
            (40, up, None, true),
        ] {
            assert_eq!(
                input.recognize(gesture, &packet, at, false, |_| live_chord),
                expected
            );
        }
    }

    #[test]
    fn raw_chords_alt_tab_altgr_mouse_and_prior_held_keys_interrupt() {
        let left = key(VK_MENU, 0x38, 0);
        let up = key(VK_MENU, 0x38, RI_KEY_BREAK);
        let click = RAWINPUT {
            header: RAWINPUTHEADER {
                dwType: RIM_TYPEMOUSE,
                ..Default::default()
            },
            data: RAWINPUT_0 {
                mouse: RAWMOUSE {
                    Anonymous: RAWMOUSE_0 {
                        Anonymous: RAWMOUSE_0_0 {
                            usButtonFlags: RI_MOUSE_LEFT_BUTTON_DOWN as u16,
                            ..Default::default()
                        },
                    },
                    ..Default::default()
                },
            },
        };
        let gesture = Gesture::parse("double-left-alt").unwrap().unwrap();
        for interrupt in [key(VK_TAB, 0x0f, 0), key(VK_MENU, 0x38, RI_KEY_E0), click] {
            let mut input = GestureInput::default();
            for (at, packet) in [(0, left), (20, up), (50, left), (55, interrupt), (60, up)] {
                assert!(!input.recognize(gesture, &packet, at, false, |_| None));
            }
        }
        // An already-held Ctrl also represents AltGr on layouts which emulate Ctrl+Alt.
        let right = Gesture::parse("double-right-alt").unwrap().unwrap();
        let make = key(VK_MENU, 0x38, RI_KEY_E0);
        let release = key(VK_MENU, 0x38, RI_KEY_E0 | RI_KEY_BREAK);
        let mut input = GestureInput::default();
        for (at, packet, held) in [
            (0, make, true),
            (10, make, false),
            (20, release, false),
            (40, make, false),
            (60, release, false),
        ] {
            assert!(!input.recognize(right, &packet, at, false, |_| Some(held)));
        }
    }
}
