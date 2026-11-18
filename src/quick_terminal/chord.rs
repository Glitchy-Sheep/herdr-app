//! The portable subset of global shortcuts understood by the native backends.

#[derive(Debug, PartialEq)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
    pub key: Key,
}

#[derive(Debug, PartialEq)]
pub enum Key {
    Character(u8),
    Function(u8),
    Backquote,
    Space,
    Escape,
    Tab,
    Enter,
}

pub fn parse(text: &str) -> Result<Chord, String> {
    let text = text.trim().to_ascii_lowercase();
    let mut parts: Vec<_> = text.split(['-', '+']).map(str::trim).collect();
    if parts.first() == Some(&"double") {
        return Err("quick_terminal.shortcut: choose an ordinary chord such as ctrl-alt-backquote or a function key".into());
    }
    if parts.iter().any(|part| {
        matches!(
            *part,
            "left"
                | "right"
                | "lcmd"
                | "rcmd"
                | "lctrl"
                | "rctrl"
                | "lcontrol"
                | "rcontrol"
                | "lalt"
                | "ralt"
                | "loption"
                | "roption"
                | "lshift"
                | "rshift"
                | "lsuper"
                | "rsuper"
                | "lwin"
                | "rwin"
        )
    }) {
        return Err(
            "quick_terminal.shortcut: ordinary chords use ctrl, alt, shift or super without a side"
                .into(),
        );
    }
    let key = match parts.pop().unwrap_or("") {
        "backquote" | "grave" | "`" => Key::Backquote,
        "space" => Key::Space,
        "escape" | "esc" => Key::Escape,
        "tab" => Key::Tab,
        "enter" | "return" => Key::Enter,
        key if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() => Key::Character(key.as_bytes()[0]),
        key if key.starts_with('f') => match key[1..].parse::<u8>() {
            Ok(number @ 1..=24) => Key::Function(number),
            _ => return Err("quick_terminal.shortcut: function keys must be F1 through F24".into()),
        },
        _ => return Err("quick_terminal.shortcut: use F1-F24, or modifiers and a letter, digit, backquote, space, escape, tab or enter".into()),
    };
    let mut chord = Chord {
        ctrl: false,
        alt: false,
        shift: false,
        super_key: false,
        key,
    };
    for modifier in parts {
        let flag = match modifier {
            "ctrl" | "control" => &mut chord.ctrl,
            "alt" | "option" => &mut chord.alt,
            "shift" => &mut chord.shift,
            "super" | "win" => &mut chord.super_key,
            _ => {
                return Err(format!(
                    "quick_terminal.shortcut: unknown modifier {modifier:?}; use ctrl, alt, shift or super"
                ));
            }
        };
        if *flag {
            return Err(format!(
                "quick_terminal.shortcut: repeated modifier {modifier:?}"
            ));
        }
        *flag = true;
    }
    if !chord.ctrl && !chord.alt && !chord.super_key && !matches!(chord.key, Key::Function(_)) {
        return Err("quick_terminal.shortcut needs ctrl, alt or super, or a function key, so ordinary typing is not captured".into());
    }
    Ok(chord)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_custom_chords() {
        assert_eq!(
            parse("ctrl-alt-backquote").unwrap(),
            Chord {
                ctrl: true,
                alt: true,
                shift: false,
                super_key: false,
                key: Key::Backquote,
            }
        );
        assert_eq!(parse("Super-Shift-F24").unwrap().key, Key::Function(24));
        assert_eq!(parse("alt-A").unwrap().key, Key::Character(b'a'));
        assert_eq!(parse("control-space").unwrap().key, Key::Space);
        assert_eq!(
            parse(" Ctrl + Alt + Space ").unwrap(),
            parse("ctrl-alt-space").unwrap()
        );
        for shortcut in ["f1", "F24", "shift-f8"] {
            assert!(parse(shortcut).is_ok(), "{shortcut}");
        }
    }

    #[test]
    fn rejects_typing_and_ambiguous_shortcuts() {
        for text in [
            "",
            "a",
            "shift-a",
            "ctrl-control-a",
            "ctrl-f25",
            "ctrl-",
            "double-cmd",
            "cmd-a",
            "ctrl-alt-a-b",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn rejects_platform_specific_modifiers_without_broadening_the_shortcut() {
        for shortcut in [
            "double-cmd",
            "double-left-cmd",
            "double-right-shift",
            "left-cmd-space",
            "ctrl-right-alt-a",
            "rcmd+space",
            "lshift-f8",
        ] {
            let error = parse(shortcut).unwrap_err();
            assert!(error.contains("ordinary chord"), "{shortcut}: {error}");
        }
    }
}
