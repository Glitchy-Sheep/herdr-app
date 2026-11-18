//! Timing and physical identity of an isolated Windows modifier gesture.

const MAX_PRESS_MS: u32 = 300;
pub(super) const MAX_GAP_MS: u32 = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub(crate) enum ModifierKey {
    LeftAlt = 0xa4,
    RightAlt = 0xa5,
    LeftCtrl = 0xa2,
    RightCtrl = 0xa3,
    LeftShift = 0xa0,
    RightShift = 0xa1,
    LeftWin = 0x5b,
    RightWin = 0x5c,
}

impl ModifierKey {
    pub(crate) fn from_vk(vk: u16) -> Option<Self> {
        use ModifierKey::*;
        [
            LeftAlt, RightAlt, LeftCtrl, RightCtrl, LeftShift, RightShift, LeftWin, RightWin,
        ]
        .into_iter()
        .find(|key| *key as u16 == vk)
    }

    pub(crate) fn shortcut(self) -> &'static str {
        use ModifierKey::*;
        match self {
            LeftAlt => "double-left-alt",
            RightAlt => "double-right-alt",
            LeftCtrl => "double-left-ctrl",
            RightCtrl => "double-right-ctrl",
            LeftShift => "double-left-shift",
            RightShift => "double-right-shift",
            LeftWin => "double-left-super",
            RightWin => "double-right-super",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Gesture(ModifierKey, ModifierKey);

impl Gesture {
    pub(super) fn parse(text: &str) -> Result<Option<Self>, String> {
        use ModifierKey::*;
        let text = text.trim().to_ascii_lowercase();
        let Some(modifier) = text.strip_prefix("double-") else {
            return Ok(None);
        };
        let (side, family) = modifier.split_once('-').unwrap_or(("", modifier));
        let (left, right) =
            match family {
                "alt" | "option" => (LeftAlt, RightAlt),
                "ctrl" | "control" => (LeftCtrl, RightCtrl),
                "shift" => (LeftShift, RightShift),
                "win" | "super" | "cmd" => (LeftWin, RightWin),
                _ => return Err(
                    "For a double tap choose Alt, Ctrl, Shift or Win, for example double-left-alt"
                        .into(),
                ),
            };
        Ok(Some(match side {
            "" => Self(left, right),
            "left" => Self(left, left),
            "right" => Self(right, right),
            _ => return Err("The double-tap modifier side must be left or right".into()),
        }))
    }

    pub(super) fn accepts(self, key: ModifierKey) -> bool {
        key == self.0 || key == self.1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TapPhase {
    Idle,
    Down(ModifierKey),
    Waiting(ModifierKey),
}

#[derive(Clone, Copy, Default)]
enum State {
    #[default]
    Idle,
    Held {
        key: ModifierKey,
        at: u32,
        second: bool,
    },
    Waiting {
        key: ModifierKey,
        at: u32,
    },
    Blocked(ModifierKey),
}

#[derive(Clone, Default)]
pub(crate) struct DoubleTap(State);

impl DoubleTap {
    pub(crate) fn phase(&self) -> TapPhase {
        match self.0 {
            State::Idle | State::Blocked(_) => TapPhase::Idle,
            State::Held { key, .. } => TapPhase::Down(key),
            State::Waiting { key, .. } => TapPhase::Waiting(key),
        }
    }

    pub(super) fn can_start_press(&self) -> bool {
        matches!(self.0, State::Idle | State::Waiting { .. })
    }

    pub(crate) fn interrupt(&mut self) {
        self.0 = match self.0 {
            State::Held { key, .. } | State::Blocked(key) => State::Blocked(key),
            _ => State::Idle,
        };
    }

    pub(crate) fn refresh(&mut self, time: u32) {
        match self.0 {
            State::Held { at, .. } if time.wrapping_sub(at) > MAX_PRESS_MS => self.interrupt(),
            State::Waiting { at, .. } if time.wrapping_sub(at) > MAX_GAP_MS => self.0 = State::Idle,
            _ => {}
        }
    }

    /// The Win32 message timestamp is milliseconds since startup and wraps at 32 bits.
    pub(crate) fn key(&mut self, key: ModifierKey, down: bool, time: u32) -> bool {
        self.refresh(time);
        match (self.0, down) {
            (State::Held { key: held, .. }, true) if held != key => self.interrupt(),
            (State::Held { .. } | State::Blocked(_), true) => {}
            (state, true) => {
                let second = matches!(state, State::Waiting { key: first, .. } if first == key);
                self.0 = State::Held {
                    key,
                    at: time,
                    second,
                };
            }
            (
                State::Held {
                    key: held, second, ..
                },
                false,
            ) if held == key => {
                self.0 = if second {
                    State::Idle
                } else {
                    State::Waiting { key, at: time }
                };
                return second;
            }
            (State::Blocked(held), false) if held == key => self.0 = State::Idle,
            (_, false) => self.interrupt(),
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{DoubleTap, Gesture, ModifierKey::*, TapPhase};

    #[test]
    fn physical_side_release_and_wrapping_message_time() {
        let mut gesture = DoubleTap::default();
        for (key, down, time) in [
            (LeftAlt, true, u32::MAX - 100),
            (LeftAlt, false, u32::MAX - 20),
            (RightAlt, true, 10),
            (RightAlt, false, 40),
            (RightAlt, true, 90),
        ] {
            assert!(!gesture.key(key, down, time));
        }
        assert!(gesture.key(RightAlt, false, 120));
        assert_eq!(gesture.phase(), TapPhase::Idle);
        assert!(!gesture.key(LeftAlt, true, 300));
        assert!(!gesture.key(LeftAlt, false, 320));
        assert!(!gesture.key(LeftAlt, true, 350));
        assert!(gesture.key(LeftAlt, false, 380));
    }

    #[test]
    fn repeats_holds_intervening_input_and_expired_pairs() {
        let mut tap = DoubleTap::default();
        assert!(!tap.key(LeftAlt, true, 0));
        assert!(!tap.key(LeftAlt, true, 190));
        assert!(!tap.key(LeftAlt, false, 320));
        assert_eq!(tap.phase(), TapPhase::Idle);
        assert!(!tap.key(LeftAlt, true, 400));
        tap.interrupt();
        assert!(!tap.key(LeftAlt, true, 420));
        assert!(!tap.key(LeftAlt, false, 480));
        assert_eq!(tap.phase(), TapPhase::Idle);
        assert!(!tap.key(LeftAlt, true, 500));
        assert!(!tap.key(LeftAlt, false, 540));
        assert!(!tap.key(LeftAlt, true, 941));
        assert!(!tap.key(LeftAlt, false, 970));
        tap.interrupt();
        assert_eq!(tap.phase(), TapPhase::Idle);
    }

    #[test]
    fn double_modifiers_accept_an_optional_side() {
        for (name, left, right) in [
            ("alt", LeftAlt, RightAlt),
            ("ctrl", LeftCtrl, RightCtrl),
            ("shift", LeftShift, RightShift),
            ("super", LeftWin, RightWin),
        ] {
            assert!(
                Gesture::parse(left.shortcut())
                    .unwrap()
                    .unwrap()
                    .accepts(left)
            );
            assert!(
                !Gesture::parse(left.shortcut())
                    .unwrap()
                    .unwrap()
                    .accepts(right)
            );
            let either = Gesture::parse(&format!("double-{name}")).unwrap().unwrap();
            assert!(either.accepts(left) && either.accepts(right));
        }
        assert!(Gesture::parse("double-left-a").is_err());
        assert!(Gesture::parse("double-third-alt").is_err());
        assert!(Gesture::parse("ctrl-alt-f8").unwrap().is_none());
    }
}
