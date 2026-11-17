//! Double modifier taps retain physical identity; ordinary chords accept either side.

use std::collections::BTreeSet;

pub const MODIFIERS: u64 = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20);
const FUNCTION: u64 = 1 << 23;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Control,
    Alt,
    Shift,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Either,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modifier {
    pub family: Family,
    pub side: Side,
}

impl Modifier {
    pub fn from_key(key: u16) -> Option<Self> {
        let (family, side) = match key {
            59 => (Family::Control, Side::Left),
            62 => (Family::Control, Side::Right),
            58 => (Family::Alt, Side::Left),
            61 => (Family::Alt, Side::Right),
            56 => (Family::Shift, Side::Left),
            60 => (Family::Shift, Side::Right),
            55 => (Family::Command, Side::Left),
            54 => (Family::Command, Side::Right),
            _ => return None,
        };
        Some(Self { family, side })
    }

    pub fn flag(self) -> u64 {
        match self.family {
            Family::Control => 1 << 18,
            Family::Alt => 1 << 19,
            Family::Shift => 1 << 17,
            Family::Command => 1 << 20,
        }
    }

    pub fn name(self) -> String {
        let side = match self.side {
            Side::Either => "",
            Side::Left => "left-",
            Side::Right => "right-",
        };
        let family = match self.family {
            Family::Control => "ctrl",
            Family::Alt => "alt",
            Family::Shift => "shift",
            Family::Command => "cmd",
        };
        format!("{side}{family}")
    }
}

fn modifier(name: &str) -> Option<Modifier> {
    let (side, name) = if let Some(name) = name
        .strip_prefix("left-")
        .or_else(|| name.strip_prefix("left_"))
    {
        (Side::Left, name)
    } else if let Some(name) = name
        .strip_prefix("right-")
        .or_else(|| name.strip_prefix("right_"))
    {
        (Side::Right, name)
    } else if matches!(
        name,
        "lcmd"
            | "lcommand"
            | "lctrl"
            | "lcontrol"
            | "lalt"
            | "loption"
            | "lshift"
            | "lsuper"
            | "lmeta"
    ) {
        (Side::Left, &name[1..])
    } else if matches!(
        name,
        "rcmd"
            | "rcommand"
            | "rctrl"
            | "rcontrol"
            | "ralt"
            | "roption"
            | "rshift"
            | "rsuper"
            | "rmeta"
    ) {
        (Side::Right, &name[1..])
    } else {
        (Side::Either, name)
    };
    let family = match name {
        "ctrl" | "control" => Family::Control,
        "alt" | "option" | "opt" => Family::Alt,
        "shift" => Family::Shift,
        "cmd" | "command" | "super" | "meta" => Family::Command,
        _ => return None,
    };
    Some(Modifier { family, side })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Double(Modifier),
    Chord(Chord),
}

impl Shortcut {
    pub fn parse(text: &str) -> Result<Self, String> {
        let normalized = text.trim().to_ascii_lowercase();
        if let Some(name) = normalized.strip_prefix("double-") {
            return modifier(name).map(Self::Double).ok_or_else(|| {
                "Double tap needs Cmd, Ctrl, Alt or Shift, optionally left or right".into()
            });
        }
        let normalized = normalized
            .replace("left-", "left_")
            .replace("right-", "right_");
        let parts: Vec<_> = normalized
            .split(if normalized.contains('+') { '+' } else { '-' })
            .map(str::trim)
            .collect();
        let (key, modifiers) = parts
            .split_last()
            .ok_or("A quick-terminal shortcut is required")?;
        let key = key_code(key).ok_or_else(|| format!("Unsupported quick-terminal key '{key}'"))?;
        let mut parsed = Vec::new();
        for name in modifiers {
            let value =
                modifier(name).ok_or_else(|| format!("Unknown shortcut modifier '{name}'"))?;
            if value.side != Side::Either {
                return Err("Left and right modifiers are supported only for double taps".into());
            }
            if parsed
                .iter()
                .any(|previous: &Modifier| previous.family == value.family)
            {
                return Err(format!("Repeated shortcut modifier '{name}'"));
            }
            parsed.push(value);
        }
        if !parsed
            .iter()
            .any(|modifier| modifier.family != Family::Shift)
            && !is_function_key(key)
        {
            return Err("A global shortcut needs Cmd, Ctrl or Alt, a function key, or a double modifier tap".into());
        }
        parsed.sort_by_key(|modifier| modifier.family as u8);
        Ok(Self::Chord(Chord {
            key,
            modifiers: parsed,
        }))
    }

    pub fn name(&self) -> String {
        match self {
            Self::Double(modifier) => format!("double-{}", modifier.name()),
            Self::Chord(chord) => chord.name(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub key: u16,
    pub modifiers: Vec<Modifier>,
}

impl Chord {
    pub fn carbon_flags(&self) -> u32 {
        self.modifiers.iter().fold(0, |flags, modifier| {
            flags
                | match modifier.family {
                    Family::Command => 1 << 8,
                    Family::Shift => 1 << 9,
                    Family::Alt => 1 << 11,
                    Family::Control => 1 << 12,
                }
        })
    }

    pub fn record(key: u16, flags: u64) -> Result<Self, String> {
        if key_name(key).is_none() {
            return Err("This key cannot be used as a quick-terminal shortcut".into());
        }
        if flags & FUNCTION != 0 && !is_function_key(key) && !matches!(key, 123..=126) {
            return Err("The Fn modifier cannot be used in a quick-terminal shortcut".into());
        }
        let mut modifiers = Vec::new();
        for family in [Family::Control, Family::Alt, Family::Shift, Family::Command] {
            let modifier = Modifier {
                family,
                side: Side::Either,
            };
            if flags & modifier.flag() == 0 {
                continue;
            }
            modifiers.push(modifier);
        }
        if !modifiers
            .iter()
            .any(|modifier| modifier.family != Family::Shift)
            && !is_function_key(key)
        {
            return Err(
                "Hold Cmd, Ctrl or Alt with the key, press F1-F20, or tap one modifier twice"
                    .into(),
            );
        }
        Ok(Self { key, modifiers })
    }

    pub fn name(&self) -> String {
        let mut parts: Vec<_> = self
            .modifiers
            .iter()
            .map(|modifier| modifier.name())
            .collect();
        parts.push(key_name(self.key).unwrap_or("unknown").into());
        parts.join("-")
    }
}

/// A tap pair uses the same physical key, with no intervening input.
pub struct DoubleTap {
    modifier: Modifier,
    pressed: Option<(u16, f64)>,
    released: Option<(u16, f64)>,
    down: bool,
    blocked: bool,
    held_keys: BTreeSet<u16>,
}

impl DoubleTap {
    pub fn new(modifier: Modifier) -> Self {
        Self {
            modifier,
            pressed: None,
            released: None,
            down: false,
            blocked: false,
            held_keys: BTreeSet::new(),
        }
    }

    pub fn ordinary_key(&mut self, key: u16, down: bool) {
        if down {
            self.held_keys.insert(key);
        } else {
            self.held_keys.remove(&key);
        }
        self.cancel();
    }

    pub fn cancel(&mut self) {
        self.pressed = None;
        self.released = None;
        self.blocked = self.down;
    }

    pub fn flags_with_key_state(
        &mut self,
        key: u16,
        flags: u64,
        at: f64,
        mut still_held: impl FnMut(u16) -> bool,
    ) -> bool {
        if !self.down && flags & self.modifier.flag() != 0 {
            self.held_keys.retain(|key| still_held(*key));
        }
        self.flags(key, flags, at)
    }

    pub fn flags(&mut self, key: u16, flags: u64, at: f64) -> bool {
        let was_down = self.down;
        self.down = flags & self.modifier.flag() != 0;
        let matches = Modifier::from_key(key).is_some_and(|key| {
            key.family == self.modifier.family
                && (self.modifier.side == Side::Either || key.side == self.modifier.side)
        });
        if !self.held_keys.is_empty()
            || !matches
            || flags & (MODIFIERS | FUNCTION) & !self.modifier.flag() != 0
        {
            self.cancel();
            return false;
        }
        if self.blocked {
            self.blocked = self.down;
            return false;
        }
        if self.down {
            if was_down {
                self.cancel();
            } else {
                self.pressed = Some((key, at));
            }
            return false;
        }
        let Some((pressed_key, pressed)) = self.pressed.take() else {
            return false;
        };
        if key != pressed_key || at < pressed || at - pressed > 0.3 {
            self.released = None;
            return false;
        }
        let double = self
            .released
            .take()
            .is_some_and(|(last_key, last)| key == last_key && pressed >= last && at - last <= 0.4);
        if !double {
            self.released = Some((key, at));
        }
        double
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaptureResult {
    Recording,
    Recorded(String),
    Cancelled,
    Error(String),
}

pub struct Capture {
    pub result: CaptureResult,
    pub preview: Option<String>,
    detector: Option<(Modifier, DoubleTap)>,
    held_keys: BTreeSet<u16>,
    held_modifiers: u64,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            result: CaptureResult::Recording,
            preview: None,
            detector: None,
            held_keys: BTreeSet::new(),
            held_modifiers: 0,
        }
    }
}

impl Capture {
    pub fn cancel(&mut self) {
        self.result = CaptureResult::Cancelled;
    }

    pub fn reset_taps(&mut self) {
        self.detector = None;
        self.preview = None;
    }

    pub fn ready(&self) -> bool {
        matches!(self.result, CaptureResult::Recorded(_)) && self.drained()
    }

    pub fn drained(&self) -> bool {
        self.held_keys.is_empty() && self.held_modifiers == 0
    }

    pub fn phase(&self) -> &'static str {
        match self.result {
            CaptureResult::Recorded(_) => {
                if self.ready() {
                    "complete"
                } else {
                    "releasing"
                }
            }
            CaptureResult::Error(_) if !self.drained() => "releasing",
            CaptureResult::Recording if self.held_modifiers != 0 => "modifier-down",
            CaptureResult::Recording
                if self
                    .detector
                    .as_ref()
                    .is_some_and(|(_, detector)| detector.released.is_some()) =>
            {
                "awaiting-second-tap"
            }
            _ => "idle",
        }
    }

    pub fn refresh(&mut self, at: f64) {
        if let Some((_, detector)) = &mut self.detector
            && detector.released.is_some_and(|(_, last)| at - last > 0.4)
        {
            detector.released = None;
            if self.held_modifiers == 0 {
                self.preview = None;
            }
        }
    }

    pub fn set_modifiers(&mut self, flags: u64) {
        self.held_modifiers = flags & (MODIFIERS | FUNCTION);
    }

    fn key_modifiers(&mut self, flags: u64) {
        // F-key events carry the Fn flag even when the physical Fn key is released.
        self.held_modifiers = (self.held_modifiers & FUNCTION) | (flags & MODIFIERS);
    }

    pub fn key_down(&mut self, key: u16, flags: u64, repeated: bool) -> bool {
        self.key_modifiers(flags);
        if self.result == CaptureResult::Cancelled {
            return false;
        }
        let was_held = !self.held_keys.insert(key);
        if was_held {
            return true;
        }
        if key == 53 {
            self.cancel();
            return true;
        }
        if self.result != CaptureResult::Recording {
            return true;
        }
        self.reset_taps();
        if repeated {
            return true;
        }
        self.result = match Chord::record(key, flags) {
            Ok(chord) => CaptureResult::Recorded(chord.name()),
            Err(error) => CaptureResult::Error(error),
        };
        true
    }

    pub fn key_up(&mut self, key: u16, flags: u64) -> bool {
        self.key_modifiers(flags);
        if self.held_keys.remove(&key) {
            return true;
        }
        if self.result != CaptureResult::Recording {
            return self.result != CaptureResult::Cancelled;
        }
        self.reset_taps();
        true
    }

    pub fn flags(&mut self, key: u16, flags: u64, at: f64) -> bool {
        self.set_modifiers(flags);
        if self.result != CaptureResult::Recording {
            return self.result != CaptureResult::Cancelled;
        }
        if !self.held_keys.is_empty() {
            self.reset_taps();
            return true;
        }
        let Some(modifier) = Modifier::from_key(key) else {
            self.reset_taps();
            return true;
        };
        if flags & (MODIFIERS | FUNCTION) != 0 && flags & (MODIFIERS | FUNCTION) != modifier.flag()
        {
            self.reset_taps();
            return true;
        }
        if flags & modifier.flag() != 0
            && self
                .detector
                .as_ref()
                .is_none_or(|(current, _)| *current != modifier)
        {
            self.detector = Some((modifier, DoubleTap::new(modifier)));
        }
        if let Some((current, detector)) = &mut self.detector {
            if detector.flags(key, flags, at) {
                self.result = CaptureResult::Recorded(format!("double-{}", current.name()));
            }
            self.preview = (detector.pressed.is_some()
                || detector.released.is_some()
                || matches!(self.result, CaptureResult::Recorded(_)))
            .then(|| current.name());
        }
        true
    }
}

pub fn key_code(name: &str) -> Option<u16> {
    Some(match name {
        "a" => 0,
        "s" => 1,
        "d" => 2,
        "f" => 3,
        "h" => 4,
        "g" => 5,
        "z" => 6,
        "x" => 7,
        "c" => 8,
        "v" => 9,
        "b" => 11,
        "q" => 12,
        "w" => 13,
        "e" => 14,
        "r" => 15,
        "y" => 16,
        "t" => 17,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "=" | "equal" => 24,
        "9" => 25,
        "7" => 26,
        "minus" => 27,
        "8" => 28,
        "0" => 29,
        "]" | "bracketright" => 30,
        "o" => 31,
        "u" => 32,
        "[" | "bracketleft" => 33,
        "i" => 34,
        "p" => 35,
        "enter" | "return" => 36,
        "l" => 37,
        "j" => 38,
        "'" | "quote" => 39,
        "k" => 40,
        ";" | "semicolon" => 41,
        "\\" | "backslash" => 42,
        "," | "comma" => 43,
        "/" | "slash" => 44,
        "n" => 45,
        "m" => 46,
        "." | "period" => 47,
        "tab" => 48,
        "space" => 49,
        "`" | "grave" | "backtick" | "backquote" => 50,
        "backspace" => 51,
        "escape" | "esc" => 53,
        "f1" => 122,
        "f2" => 120,
        "f3" => 99,
        "f4" => 118,
        "f5" => 96,
        "f6" => 97,
        "f7" => 98,
        "f8" => 100,
        "f9" => 101,
        "f10" => 109,
        "f11" => 103,
        "f12" => 111,
        "f13" => 105,
        "f14" => 107,
        "f15" => 113,
        "f16" => 106,
        "f17" => 64,
        "f18" => 79,
        "f19" => 80,
        "f20" => 90,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        _ => return None,
    })
}

pub fn key_name(key: u16) -> Option<&'static str> {
    const NAMES: &[&str] = &[
        "a",
        "s",
        "d",
        "f",
        "h",
        "g",
        "z",
        "x",
        "c",
        "v",
        "b",
        "q",
        "w",
        "e",
        "r",
        "y",
        "t",
        "1",
        "2",
        "3",
        "4",
        "6",
        "5",
        "equal",
        "9",
        "7",
        "minus",
        "8",
        "0",
        "bracketright",
        "o",
        "u",
        "bracketleft",
        "i",
        "p",
        "enter",
        "l",
        "j",
        "quote",
        "k",
        "semicolon",
        "backslash",
        "comma",
        "slash",
        "n",
        "m",
        "period",
        "tab",
        "space",
        "backquote",
        "backspace",
        "escape",
        "f1",
        "f2",
        "f3",
        "f4",
        "f5",
        "f6",
        "f7",
        "f8",
        "f9",
        "f10",
        "f11",
        "f12",
        "f13",
        "f14",
        "f15",
        "f16",
        "f17",
        "f18",
        "f19",
        "f20",
        "left",
        "right",
        "down",
        "up",
    ];
    NAMES
        .iter()
        .copied()
        .find(|name| key_code(name) == Some(key))
}

fn is_function_key(key: u16) -> bool {
    matches!(
        key,
        122 | 120
            | 99
            | 118
            | 96
            | 97
            | 98
            | 100
            | 101
            | 109
            | 103
            | 111
            | 105
            | 107
            | 113
            | 106
            | 64
            | 79
            | 80
            | 90
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct ModifierCase {
        name: &'static str,
        family: Family,
        keys: [u16; 2],
        flag: u64,
        device_flags: [u64; 2],
        carbon_flag: u32,
    }

    const MODIFIER_CASES: [ModifierCase; 4] = [
        ModifierCase {
            name: "ctrl",
            family: Family::Control,
            keys: [59, 62],
            flag: 1 << 18,
            device_flags: [0x1, 0x2000],
            carbon_flag: 1 << 12,
        },
        ModifierCase {
            name: "alt",
            family: Family::Alt,
            keys: [58, 61],
            flag: 1 << 19,
            device_flags: [0x20, 0x40],
            carbon_flag: 1 << 11,
        },
        ModifierCase {
            name: "shift",
            family: Family::Shift,
            keys: [56, 60],
            flag: 1 << 17,
            device_flags: [0x2, 0x4],
            carbon_flag: 1 << 9,
        },
        ModifierCase {
            name: "cmd",
            family: Family::Command,
            keys: [55, 54],
            flag: 1 << 20,
            device_flags: [0x8, 0x10],
            carbon_flag: 1 << 8,
        },
    ];

    fn detector(text: &str) -> DoubleTap {
        let Shortcut::Double(modifier) = Shortcut::parse(text).unwrap() else {
            panic!()
        };
        DoubleTap::new(modifier)
    }

    fn tap(detector: &mut DoubleTap, key: u16, at: f64) -> bool {
        let modifier = Modifier::from_key(key).unwrap();
        assert!(!detector.flags(key, modifier.flag(), at));
        detector.flags(key, 0, at + 0.05)
    }

    #[test]
    fn physical_side_and_family_are_required_for_double_taps() {
        for (name, left, right) in [
            ("cmd", 55, 54),
            ("ctrl", 59, 62),
            ("alt", 58, 61),
            ("shift", 56, 60),
        ] {
            let mut d = detector(&format!("double-left-{name}"));
            assert!(!tap(&mut d, right, 1.0));
            assert!(!tap(&mut d, right, 1.2));
            assert!(!tap(&mut d, left, 1.4));
            assert!(tap(&mut d, left, 1.6));
            let mut d = detector(&format!("double-{name}"));
            assert!(!tap(&mut d, left, 2.0));
            assert!(!tap(&mut d, right, 2.2));
            assert!(tap(&mut d, right, 2.4));
        }
    }

    #[test]
    fn global_double_taps_wait_until_all_typing_keys_are_released() {
        let mut d = detector("double-left-alt");
        d.ordinary_key(0, true);
        d.ordinary_key(0, true);
        d.ordinary_key(12, true);
        assert!(!tap(&mut d, 58, 1.0));
        assert!(!tap(&mut d, 58, 1.1));
        d.ordinary_key(0, false);
        assert!(!tap(&mut d, 58, 2.0));
        assert!(!tap(&mut d, 58, 2.1));
        d.ordinary_key(12, false);
        assert!(!tap(&mut d, 58, 3.0));
        assert!(tap(&mut d, 58, 3.1));
    }

    #[test]
    fn missing_key_ups_recover_without_counting_an_overlapping_tap() {
        let mut d = detector("double-left-alt");
        d.ordinary_key(0, true);
        d.ordinary_key(12, true);
        let alt = 1 << 19;
        // A was released without a key-up. Q stays down across two taps.
        for at in [1.0, 1.15] {
            assert!(!d.flags_with_key_state(58, alt, at, |key| key == 12));
            assert!(!d.flags(58, 0, at + 0.05));
        }
        assert!(!d.flags_with_key_state(58, alt, 1.3, |key| key == 12));
        // Q is released during this tap. Both key-ups are now missing.
        assert!(!d.flags(58, 0, 1.35));
        assert!(!d.flags_with_key_state(58, alt, 1.45, |key| {
            assert_eq!(key, 12);
            false
        }));
        assert!(!d.flags(58, 0, 1.5));
        assert!(!d.flags_with_key_state(58, alt, 1.6, |_| panic!("no tracked keys remain")));
        assert!(d.flags(58, 0, 1.65));
    }

    #[test]
    fn shortcut_keys_other_modifiers_and_holds_cancel_double_taps() {
        let mut d = detector("double-cmd");
        assert!(!tap(&mut d, 55, 1.0));
        d.flags(55, 1 << 20, 1.1);
        d.cancel();
        assert!(!d.flags(55, 0, 1.15));
        assert!(!tap(&mut d, 55, 1.2));
        d.flags(56, (1 << 20) | (1 << 17), 1.3);
        assert!(!tap(&mut d, 55, 1.4));
        assert!(!tap(&mut d, 55, 2.0));
        d.flags(55, 1 << 20, 2.1);
        assert!(!d.flags(55, 0, 3.0));
        assert!(!tap(&mut d, 55, 3.1));
    }

    #[test]
    fn overlapping_left_and_right_keys_cancel_the_gesture() {
        let mut d = detector("double-cmd");
        assert!(!tap(&mut d, 55, 1.0));
        d.flags(55, 1 << 20, 1.1);
        d.flags(54, 1 << 20, 1.15);
        assert!(!d.flags(55, 1 << 20, 1.2));
        assert!(!d.flags(54, 0, 1.25));
        assert!(!tap(&mut d, 55, 1.3));
    }

    #[test]
    fn regular_chords_reject_side_constraints() {
        for text in ["right-cmd-left-shift-space", "lcmd-space", "left-ctrl-a"] {
            assert!(Shortcut::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn recorder_ordinary_chords_use_either_side_and_roundtrip() {
        let chord = Chord::record(49, (1 << 18) | (1 << 19) | 0x1 | 0x40).unwrap();
        assert_eq!(chord.name(), "ctrl-alt-space");
        assert_eq!(
            Shortcut::parse(&chord.name()).unwrap(),
            Shortcut::Chord(chord)
        );
        assert_eq!(
            Chord::record(49, (1 << 18) | 0x2001).unwrap().name(),
            "ctrl-space"
        );
        assert!(Chord::record(49, 0).is_err());
    }

    #[test]
    fn canonical_aliases_and_existing_chords_are_compatible() {
        for (text, canonical) in [
            ("double-rcmd", "double-right-cmd"),
            ("double-left-option", "double-left-alt"),
            ("ctrl-alt-backquote", "ctrl-alt-backquote"),
            ("Cmd+Shift+`", "shift-cmd-backquote"),
        ] {
            assert_eq!(Shortcut::parse(text).unwrap().name(), canonical);
        }
        for text in [
            "space",
            "cmd-mystery",
            "hyper-space",
            "cmd-left-cmd-space",
            "double-fn",
            "cmd-",
        ] {
            assert!(Shortcut::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn standalone_function_keys_are_allowed_but_typing_keys_need_modifiers() {
        for number in 1..=20 {
            let text = format!("f{number}");
            assert_eq!(Shortcut::parse(&text).unwrap().name(), text);
            assert_eq!(
                Chord::record(key_code(&text).unwrap(), 0).unwrap().name(),
                text
            );
            assert_eq!(
                Chord::record(key_code(&text).unwrap(), FUNCTION)
                    .unwrap()
                    .name(),
                text
            );
        }
        for text in ["a", "space", "enter", "escape", "f21"] {
            assert!(Shortcut::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn capture_autodetects_double_taps_and_keeps_their_side() {
        let mut capture = Capture::default();
        for at in [1.0, 1.2] {
            assert!(capture.flags(54, 1 << 20, at));
            assert_eq!(capture.phase(), "modifier-down");
            assert_eq!(capture.preview.as_deref(), Some("right-cmd"));
            assert!(!capture.ready());
            assert!(capture.flags(54, 0, at + 0.05));
        }
        assert_eq!(
            capture.result,
            CaptureResult::Recorded("double-right-cmd".into())
        );
        assert!(capture.ready());
        assert_eq!(capture.phase(), "complete");
    }

    #[test]
    fn capture_chords_cancel_taps_and_consume_the_matching_key_release() {
        let mut capture = Capture::default();
        capture.flags(55, 1 << 20, 1.0);
        capture.flags(55, 0, 1.05);
        capture.flags(55, 1 << 20, 1.1);
        assert!(capture.key_down(12, 1 << 20, false));
        assert_eq!(capture.result, CaptureResult::Recorded("cmd-q".into()));
        assert!(capture.key_down(12, 1 << 20, true));
        assert!(capture.key_up(12, 1 << 20));
        assert!(capture.key_down(0, 0, false));
        assert_eq!(capture.result, CaptureResult::Recorded("cmd-q".into()));
    }

    #[test]
    fn capture_ignores_an_existing_repeat_and_escape_cancels() {
        let mut capture = Capture::default();
        assert!(capture.key_down(0, 0, true));
        assert_eq!(capture.result, CaptureResult::Recording);
        assert!(capture.key_down(53, 0, false));
        assert_eq!(capture.result, CaptureResult::Cancelled);
        assert!(capture.key_up(53, 0));
    }

    #[test]
    fn captured_chords_wait_for_key_and_modifier_releases_in_either_order() {
        for modifier_first in [true, false] {
            let mut capture = Capture::default();
            capture.key_down(12, 1 << 20, false);
            assert_eq!(capture.phase(), "releasing");
            assert!(!capture.ready());
            assert!(capture.key_down(12, 1 << 20, true));
            assert!(!capture.ready());
            if modifier_first {
                capture.flags(55, 0, 1.0);
                assert!(!capture.ready());
                capture.key_up(12, 0);
            } else {
                capture.key_up(12, 1 << 20);
                assert!(!capture.ready());
                capture.flags(55, 0, 1.0);
            }
            assert!(capture.ready());
        }
    }

    #[test]
    fn overlapping_and_preexisting_repeating_keys_must_also_be_released() {
        for preexisting in [true, false] {
            let mut capture = Capture::default();
            if preexisting {
                capture.key_down(0, 0, true);
            }
            capture.key_down(12, 1 << 20, false);
            if !preexisting {
                capture.key_down(0, 1 << 20, false);
            }
            capture.key_up(12, 1 << 20);
            capture.flags(55, 0, 1.0);
            assert!(!capture.ready());
            capture.key_up(0, 0);
            assert!(capture.ready());
        }
    }

    #[test]
    fn a_preexisting_held_key_cannot_be_ignored_for_double_modifier_capture() {
        let mut capture = Capture::default();
        capture.key_down(0, 0, true);
        for at in [1.0, 1.2] {
            capture.flags(54, 1 << 20, at);
            capture.flags(54, 0, at + 0.05);
        }
        assert_eq!(capture.result, CaptureResult::Recording);
        capture.key_up(0, 0);
        for at in [2.0, 2.2] {
            capture.flags(54, 1 << 20, at);
            capture.flags(54, 0, at + 0.05);
        }
        assert!(capture.ready());
    }

    #[test]
    fn awaiting_second_tap_preview_expires_and_a_long_hold_is_not_a_tap() {
        let mut capture = Capture::default();
        capture.flags(58, 1 << 19, 1.0);
        capture.flags(58, 0, 1.05);
        assert_eq!(capture.phase(), "awaiting-second-tap");
        assert_eq!(capture.preview.as_deref(), Some("left-alt"));
        capture.refresh(1.5);
        assert_eq!(capture.phase(), "idle");
        assert_eq!(capture.preview, None);
        capture.flags(58, 1 << 19, 2.0);
        capture.flags(58, 0, 3.0);
        assert_eq!(capture.phase(), "idle");
        assert_eq!(capture.preview, None);
    }

    #[test]
    fn function_key_event_flags_do_not_mask_a_physical_fn_hold() {
        let mut capture = Capture::default();
        capture.key_down(122, FUNCTION, false);
        capture.key_up(122, FUNCTION);
        assert!(capture.ready());
        let mut capture = Capture::default();
        capture.set_modifiers(FUNCTION);
        capture.key_down(122, FUNCTION, false);
        capture.key_up(122, FUNCTION);
        assert!(!capture.ready());
        capture.flags(63, 0, 1.0);
        assert!(capture.ready());
    }

    #[test]
    fn shift_typing_is_rejected_without_disabling_shift_function_keys() {
        for text in ["shift-a", "shift-space", "shift-enter"] {
            assert!(Shortcut::parse(text).is_err(), "{text}");
        }
        assert!(Chord::record(0, 1 << 17).is_err());
        assert_eq!(
            Chord::record(122, (1 << 17) | FUNCTION).unwrap().name(),
            "shift-f1"
        );
        assert_eq!(
            Chord::record(123, (1 << 18) | FUNCTION).unwrap().name(),
            "ctrl-left"
        );
    }

    #[test]
    fn invalid_chords_drain_without_becoming_a_saveable_result() {
        let mut capture = Capture::default();
        assert!(capture.key_down(0, 1 << 17, false));
        assert!(matches!(capture.result, CaptureResult::Error(_)));
        assert!(!capture.drained());
        assert_eq!(capture.phase(), "releasing");
        assert!(capture.key_down(0, 1 << 17, true));
        assert!(capture.key_up(0, 1 << 17));
        assert!(!capture.drained());
        assert!(capture.flags(56, 0, 1.0));
        assert!(capture.drained());
        assert!(!capture.ready());
        assert_eq!(capture.phase(), "idle");
    }

    #[test]
    fn every_modifier_side_records_and_replays_only_after_its_second_release() {
        for case in MODIFIER_CASES {
            for (side, label) in [(0, "left"), (1, "right")] {
                let name = format!("double-{label}-{}", case.name);
                let key = case.keys[side];
                let flags = case.flag | case.device_flags[side];
                let mut capture = Capture::default();
                for (index, at) in [1.0, 1.2].into_iter().enumerate() {
                    assert!(capture.flags(key, flags, at), "{name}: press");
                    assert_eq!(capture.phase(), "modifier-down", "{name}");
                    assert_eq!(
                        capture.preview,
                        Some(format!("{label}-{}", case.name)),
                        "{name}"
                    );
                    assert!(!capture.ready(), "{name}: held modifier");
                    assert!(capture.flags(key, 0, at + 0.05), "{name}: release");
                    assert_eq!(capture.ready(), index == 1, "{name}: release {index}");
                }
                let CaptureResult::Recorded(recorded) = &capture.result else {
                    panic!("{name}: no recorded result")
                };
                assert_eq!(recorded, &name);
                let Shortcut::Double(modifier) = Shortcut::parse(recorded).unwrap() else {
                    panic!("{name}: not a double tap")
                };
                assert_eq!(modifier.family, case.family, "{name}");
                assert_eq!(
                    modifier.side,
                    if side == 0 { Side::Left } else { Side::Right },
                    "{name}"
                );
                let mut global = DoubleTap::new(modifier);
                for (index, at) in [2.0, 2.2].into_iter().enumerate() {
                    assert!(!global.flags(key, flags, at), "{name}: global press");
                    assert_eq!(
                        global.flags(key, 0, at + 0.05),
                        index == 1,
                        "{name}: global release {index}"
                    );
                }
                assert!(
                    !global.flags(key, 0, 2.3),
                    "{name}: duplicate release must not retrigger"
                );
            }
        }
    }

    #[test]
    fn every_sided_double_rejects_opposite_and_mixed_side_taps() {
        for case in MODIFIER_CASES {
            for (side, label) in [(0, "left"), (1, "right")] {
                let name = format!("double-{label}-{}", case.name);
                let opposite = 1 - side;
                for sequence in [[opposite, opposite], [side, opposite], [opposite, side]] {
                    let mut global = detector(&name);
                    for (index, pressed_side) in sequence.into_iter().enumerate() {
                        let at = 1.0 + index as f64 * 0.2;
                        let key = case.keys[pressed_side];
                        assert!(
                            !global.flags(key, case.flag | case.device_flags[pressed_side], at),
                            "{name}: {sequence:?}"
                        );
                        assert!(!global.flags(key, 0, at + 0.05), "{name}: {sequence:?}");
                    }
                }
                let mut capture = Capture::default();
                let mut any_side = detector(&format!("double-{}", case.name));
                for (index, pressed_side) in [side, opposite].into_iter().enumerate() {
                    let at = 2.0 + index as f64 * 0.2;
                    let key = case.keys[pressed_side];
                    let flags = case.flag | case.device_flags[pressed_side];
                    capture.flags(key, flags, at);
                    capture.flags(key, 0, at + 0.05);
                    assert!(!any_side.flags(key, flags, at));
                    assert!(!any_side.flags(key, 0, at + 0.05));
                }
                assert_eq!(
                    capture.result,
                    CaptureResult::Recording,
                    "{name}: mixed sides must not record"
                );
                assert!(!capture.ready(), "{name}: mixed sides must not save");
            }
        }
    }

    #[test]
    fn every_modifier_side_rejects_overlap_long_hold_timeout_and_other_modifiers() {
        for (family_index, case) in MODIFIER_CASES.into_iter().enumerate() {
            for (side, label) in [(0, "left"), (1, "right")] {
                let name = format!("double-{label}-{}", case.name);
                let key = case.keys[side];
                let flags = case.flag | case.device_flags[side];
                let opposite = case.keys[1 - side];
                let other = MODIFIER_CASES[(family_index + 1) % MODIFIER_CASES.len()];
                let cases = [
                    (
                        "long hold",
                        vec![
                            (key, flags, 1.0),
                            (key, 0, 1.5),
                            (key, flags, 1.6),
                            (key, 0, 1.65),
                        ],
                    ),
                    (
                        "timeout",
                        vec![
                            (key, flags, 1.0),
                            (key, 0, 1.05),
                            (key, flags, 1.6),
                            (key, 0, 1.65),
                        ],
                    ),
                    (
                        "other modifier",
                        vec![
                            (key, flags, 1.0),
                            (key, 0, 1.05),
                            (other.keys[0], other.flag | other.device_flags[0], 1.1),
                            (other.keys[0], 0, 1.15),
                            (key, flags, 1.2),
                            (key, 0, 1.25),
                        ],
                    ),
                    (
                        "overlap, first released first",
                        vec![
                            (key, flags, 1.0),
                            (key, 0, 1.05),
                            (key, flags, 1.1),
                            (opposite, flags | case.device_flags[1 - side], 1.15),
                            (key, case.flag | case.device_flags[1 - side], 1.2),
                            (opposite, 0, 1.25),
                        ],
                    ),
                    (
                        "overlap, second released first",
                        vec![
                            (key, flags, 1.0),
                            (key, 0, 1.05),
                            (key, flags, 1.1),
                            (opposite, flags | case.device_flags[1 - side], 1.15),
                            (opposite, flags, 1.2),
                            (key, 0, 1.25),
                        ],
                    ),
                ];
                for (scenario, events) in cases {
                    let mut capture = Capture::default();
                    let mut global = detector(&name);
                    for (key, flags, at) in events {
                        assert!(!global.flags(key, flags, at), "{name}: {scenario}");
                        capture.flags(key, flags, at);
                        assert_eq!(
                            capture.result,
                            CaptureResult::Recording,
                            "{name}: {scenario}"
                        );
                        assert!(!capture.ready(), "{name}: {scenario}");
                    }
                }
            }
        }
    }

    #[test]
    fn all_ordinary_modifier_combinations_accept_either_side_and_wait_for_every_release() {
        for subset in 0_u8..16 {
            let selected: Vec<_> = MODIFIER_CASES
                .into_iter()
                .enumerate()
                .filter_map(|(index, case)| (subset & (1 << index) != 0).then_some(case))
                .collect();
            for mut variant in 0..3_usize.pow(selected.len() as u32) {
                let mut physical = Vec::new();
                for case in &selected {
                    let sides = match variant % 3 {
                        0 => &[0][..],
                        1 => &[1][..],
                        _ => &[0, 1][..],
                    };
                    variant /= 3;
                    for &side in sides {
                        physical.push((case.keys[side], case.flag, case.device_flags[side]));
                    }
                }
                let flags = physical
                    .iter()
                    .fold(0, |flags, (_, family, device)| flags | family | device);
                for (key, key_name) in [(0, "a"), (122, "f1")] {
                    let mut parts: Vec<_> = selected.iter().map(|case| case.name).collect();
                    parts.push(key_name);
                    let expected = parts.join("-");
                    let supported =
                        key == 122 || selected.iter().any(|case| case.family != Family::Shift);
                    let chord = Chord::record(key, flags);
                    let parsed = Shortcut::parse(&expected);
                    assert_eq!(chord.is_ok(), supported, "{expected}, flags={flags:#x}");
                    assert_eq!(parsed.is_ok(), supported, "{expected}, flags={flags:#x}");
                    if let Ok(chord) = chord {
                        assert_eq!(chord.name(), expected);
                        assert_eq!(
                            chord.carbon_flags(),
                            selected
                                .iter()
                                .fold(0, |flags, case| flags | case.carbon_flag),
                            "{expected}"
                        );
                        assert_eq!(parsed.unwrap(), Shortcut::Chord(chord));
                    }
                    for key_first in [true, false] {
                        for reverse in [true, false] {
                            let mut capture = Capture::default();
                            let mut down = 0;
                            for &(key, family, device) in &physical {
                                down |= family | device;
                                capture.flags(key, down, 1.0);
                            }
                            capture.key_down(key, flags, false);
                            assert!(!capture.ready(), "{expected}: key held");
                            if key_first {
                                capture.key_up(key, flags);
                                assert_eq!(
                                    capture.ready(),
                                    supported && physical.is_empty(),
                                    "{expected}: modifiers held"
                                );
                            }
                            let mut held = physical.clone();
                            let mut order = physical.clone();
                            if reverse {
                                order.reverse();
                            }
                            for (released, _, _) in order {
                                held.retain(|(key, _, _)| *key != released);
                                let remaining = held
                                    .iter()
                                    .fold(0, |flags, (_, family, device)| flags | family | device);
                                capture.flags(released, remaining, 1.1);
                                assert_eq!(
                                    capture.ready(),
                                    supported && key_first && held.is_empty(),
                                    "{expected}: remaining={held:?}"
                                );
                            }
                            if !key_first {
                                capture.key_up(key, 0);
                            }
                            assert!(
                                capture.drained(),
                                "{expected}: release order key_first={key_first}, reverse={reverse}"
                            );
                            assert_eq!(capture.ready(), supported, "{expected}");
                            if supported {
                                assert_eq!(
                                    capture.result,
                                    CaptureResult::Recorded(expected.clone())
                                );
                            } else {
                                // Shift-only typing stays available to ordinary application input.
                                assert!(
                                    matches!(capture.result, CaptureResult::Error(_)),
                                    "{expected}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fn_and_caps_lock_are_not_bindable_modifiers_and_caps_lock_state_does_not_block_shortcuts() {
        for text in [
            "double-fn",
            "double-left-fn",
            "double-globe",
            "double-capslock",
            "double-caps-lock",
            "fn-space",
            "globe-space",
            "capslock-space",
            "caps-lock-space",
        ] {
            assert!(Shortcut::parse(text).is_err(), "{text}");
        }
        for (key, flag) in [(63, FUNCTION), (57, 1 << 16)] {
            let mut capture = Capture::default();
            for at in [1.0, 1.2] {
                capture.flags(key, flag, at);
                capture.flags(key, 0, at + 0.05);
            }
            assert_eq!(
                capture.result,
                CaptureResult::Recording,
                "unsupported key {key}"
            );
            assert!(!capture.ready());
            for case in MODIFIER_CASES {
                for (side, label) in [(0, "left"), (1, "right")] {
                    let mut capture = Capture::default();
                    let mut global = detector(&format!("double-{label}-{}", case.name));
                    let target = case.keys[side];
                    let target_flags = case.flag | case.device_flags[side];
                    for (key, flags, at) in [
                        (target, target_flags, 2.0),
                        (target, 0, 2.05),
                        (key, flag, 2.1),
                        (key, 0, 2.15),
                        (target, target_flags, 2.2),
                        (target, 0, 2.25),
                    ] {
                        capture.flags(key, flags, at);
                        assert!(
                            !global.flags(key, flags, at),
                            "{} {label}: unsupported key must cancel the pending tap",
                            case.name
                        );
                    }
                    assert_eq!(capture.result, CaptureResult::Recording);
                    assert!(!capture.ready());
                }
            }
        }
        for case in MODIFIER_CASES {
            for (side, label) in [(0, "left"), (1, "right")] {
                let caps_lock = 1 << 16;
                let mut capture = Capture::default();
                let mut global = detector(&format!("double-{label}-{}", case.name));
                for (index, at) in [1.0, 1.2].into_iter().enumerate() {
                    let flags = case.flag | case.device_flags[side] | caps_lock;
                    capture.flags(case.keys[side], flags, at);
                    capture.flags(case.keys[side], caps_lock, at + 0.05);
                    assert!(!global.flags(case.keys[side], flags, at));
                    assert_eq!(
                        global.flags(case.keys[side], caps_lock, at + 0.05),
                        index == 1
                    );
                }
                assert!(
                    capture.ready(),
                    "{} {label}: Caps Lock is a toggle, not a held modifier",
                    case.name
                );
            }
        }
        assert_eq!(
            Chord::record(49, (1 << 20) | (1 << 16)).unwrap().name(),
            "cmd-space"
        );
        assert!(Chord::record(49, (1 << 20) | FUNCTION).is_err());
    }
}
