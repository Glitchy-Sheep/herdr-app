//! Key releases that must drain before a recorded shortcut can be saved.

use serde_json::Value;
use std::collections::BTreeSet;

pub fn released_status(mut status: Value, drain: &Drain, modifiers_down: bool) -> Value {
    if status["state"] == "recorded" {
        let ready = drain.ready(modifiers_down);
        status["ready"] = ready.into();
        status["phase"] = if ready { "complete" } else { "releasing" }.into();
    }
    status
}

#[derive(Default)]
pub struct Drain {
    pressed: BTreeSet<String>,
    pub native_source: bool,
    pub native_pending: bool,
}

impl Drain {
    /// Returns whether the keystroke can supply the non-modifier part of a chord.
    pub fn press(&mut self, key: &str) -> bool {
        let key = key.to_ascii_lowercase();
        // GPUI represents a standalone modifier release as a synthetic keystroke.
        if matches!(
            key.as_str(),
            "shift" | "ctrl" | "control" | "alt" | "platform" | "super" | "cmd" | "function"
        ) {
            return false;
        }
        self.pressed.insert(key);
        true
    }

    pub fn release(&mut self, key: &str) {
        self.pressed.remove(&key.to_ascii_lowercase());
    }

    pub fn ready(&self, modifiers_down: bool) -> bool {
        // A native grab can consume modifier releases before GPUI sees them.
        self.pressed.is_empty() && !self.native_pending && (!modifiers_down || self.native_source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_and_overlapping_keys_wait_for_every_release() {
        let mut drain = Drain::default();
        drain.press("a");
        drain.press("a");
        drain.press("space");
        drain.release("A");
        assert!(!drain.ready(false));
        drain.release("space");
        assert!(!drain.ready(true));
        assert!(drain.ready(false));
        drain.press("q");
        assert!(
            !drain.ready(false),
            "a new key during preview must revoke readiness"
        );
    }

    #[test]
    fn native_grabs_need_physical_release_and_modifiers_do_not_leave_phantom_keys() {
        let mut drain = Drain::default();
        assert!(!drain.press("control"));
        assert!(drain.ready(false));
        drain.native_pending = true;
        drain.release("f8");
        assert!(!drain.ready(false));
        drain.native_pending = false;
        assert!(drain.ready(false));
        drain.native_source = true;
        assert!(
            drain.ready(true),
            "native physical state supersedes stale GPUI modifiers"
        );
    }
}
