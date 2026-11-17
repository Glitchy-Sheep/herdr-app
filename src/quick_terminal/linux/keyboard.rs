//! Passive grabs for the shortcut on the current XKB layout.

use super::{chord::Chord, error, keysym};
use std::collections::BTreeSet;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::{
    xkb::{self, ConnectionExt as _, KeyModMap, KeySymMap},
    xproto::*,
};
use x11rb::rust_connection::RustConnection;

pub(super) const KEYBOARD: u16 = 0x100; // XkbUseCoreKbd

pub(super) fn watch_layout(connection: &RustConnection) -> Result<(), String> {
    let events = xkb::EventType::NEW_KEYBOARD_NOTIFY
        | xkb::EventType::MAP_NOTIFY
        | xkb::EventType::STATE_NOTIFY;
    let parts = xkb::MapPart::KEY_SYMS | xkb::MapPart::MODIFIER_MAP;
    connection
        .xkb_select_events(
            KEYBOARD,
            0u8.into(),
            events,
            parts,
            parts,
            &xkb::SelectEventsAux::new(),
        )
        .map_err(error)?
        .check()
        .map_err(error)
}

// XKB maps out-of-range groups by wrapping, clamping or redirecting each individual key.
fn group_symbols(key: &KeySymMap, group: usize) -> &[u32] {
    let groups = usize::from(key.group_info & 0xf);
    if groups == 0 || key.width == 0 {
        return &[];
    }
    let index = if group < groups {
        group
    } else {
        match key.group_info & 0xc0 {
            0x40 => groups - 1,
            0x80 => usize::from((key.group_info >> 4) & 3).min(groups - 1),
            _ => group % groups,
        }
    };
    let width = usize::from(key.width);
    key.syms
        .get(index * width..(index + 1) * width)
        .unwrap_or_default()
}

pub(super) struct KeyboardMap {
    first: u8,
    keys: Vec<KeySymMap>,
    modifiers: Vec<KeyModMap>,
    group: usize,
}

impl KeyboardMap {
    pub(super) fn read(connection: &RustConnection) -> Result<Self, String> {
        let request = xkb::GetMapRequest {
            device_spec: KEYBOARD,
            full: xkb::MapPart::KEY_SYMS | xkb::MapPart::MODIFIER_MAP,
            ..Default::default()
        };
        let reply = connection
            .send_trait_request_with_reply(request)
            .map_err(error)?
            .reply()
            .map_err(error)?;
        let group = connection
            .xkb_get_state(KEYBOARD)
            .map_err(error)?
            .reply()
            .map_err(error)?
            .group;
        Ok(Self {
            first: reply.first_key_sym,
            keys: reply.map.syms_rtrn.unwrap_or_default(),
            modifiers: reply.map.modmap_rtrn.unwrap_or_default(),
            group: usize::from(u8::from(group)),
        })
    }

    pub(super) fn symbols(&self) -> impl Iterator<Item = &[u32]> {
        self.keys.iter().map(|key| group_symbols(key, self.group))
    }

    fn keycodes(&self, symbol: u32) -> Vec<u8> {
        // Prefer the active layout. A Latin group can supply a shortcut while typing in Cyrillic.
        for group in std::iter::once(self.group).chain((0..4).filter(|group| *group != self.group))
        {
            for level in [0, 1] {
                let codes: Vec<_> = self
                    .keys
                    .iter()
                    .enumerate()
                    .filter_map(|(i, key)| {
                        (group_symbols(key, group).get(level) == Some(&symbol))
                            .then_some(self.first + i as u8)
                    })
                    .collect();
                if !codes.is_empty() {
                    return codes;
                }
            }
        }
        Vec::new()
    }
}

#[derive(Default, PartialEq, Eq)]
pub(super) struct Grabs(BTreeSet<(u8, u16)>);

impl Grabs {
    fn resolve(connection: &RustConnection, chord: &Chord) -> Result<Self, String> {
        let layout = KeyboardMap::read(connection)?;
        let codes = layout.keycodes(keysym(&chord.key));
        if codes.is_empty() {
            return Err("the shortcut key is absent from the X11 keyboard layout".into());
        }
        let masks_for = |wanted: &[u32]| {
            let mut masks = BTreeSet::new();
            for modifier in &layout.modifiers {
                if let Some(key) = modifier
                    .keycode
                    .checked_sub(layout.first)
                    .and_then(|i| layout.keys.get(usize::from(i)))
                    && group_symbols(key, layout.group)
                        .iter()
                        .any(|symbol| wanted.contains(symbol))
                {
                    masks.insert(u16::from(modifier.mods));
                }
            }
            masks
        };
        let mut masks = BTreeSet::from([0]);
        for (needed, keysyms) in [
            (chord.ctrl, [0xffe3, 0xffe4]),
            (chord.shift, [0xffe1, 0xffe2]),
            (chord.alt, [0xffe9, 0xffea]),
            (chord.super_key, [0xffeb, 0xffec]),
        ] {
            if !needed {
                continue;
            }
            let family = masks_for(&keysyms);
            if family.is_empty() {
                return Err("a shortcut modifier is absent from the current X11 layout".into());
            }
            // Each side can occupy a different mask; both sides together also match.
            let mut sides = BTreeSet::from([0]);
            for modifier in family {
                sides.extend(sides.iter().map(|mask| mask | modifier).collect::<Vec<_>>());
            }
            sides.remove(&0);
            masks = masks
                .iter()
                .flat_map(|mask| sides.iter().map(move |side| mask | side))
                .collect();
        }
        let locks = masks_for(&[0xff7f, 0xff14])
            .into_iter()
            .fold(u16::from(ModMask::LOCK), |a, b| a | b);
        let mut grabs = BTreeSet::new();
        for mask in masks {
            for ignored in super::lock_combinations(locks & !mask) {
                for &code in &codes {
                    grabs.insert((code, mask | ignored));
                }
            }
        }
        Ok(Self(grabs))
    }

    pub(super) fn matches(&self, event: &KeyPressEvent) -> bool {
        self.0
            .contains(&(event.detail, u16::from(event.state) & 255))
    }

    pub(super) fn update(
        &mut self,
        connection: &RustConnection,
        root: u32,
        chord: &Chord,
    ) -> Result<(), String> {
        let result = Self::resolve(connection, chord);
        if result.as_ref().is_ok_and(|desired| desired == self) {
            return Ok(());
        }
        self.clear(connection, root);
        let desired = result?;
        for (code, mask) in desired.0 {
            let result = connection
                .grab_key(
                    false,
                    root,
                    ModMask::from(mask),
                    code,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .map_err(error)
                .and_then(|cookie| cookie.check().map_err(error));
            if let Err(error) = result {
                self.clear(connection, root);
                return Err(format!(
                    "cannot register quick terminal shortcut; another application may own it: {error}"
                ));
            }
            self.0.insert((code, mask));
        }
        Ok(())
    }

    pub(super) fn clear(&mut self, connection: &RustConnection, root: u32) {
        for (code, mask) in std::mem::take(&mut self.0) {
            let _ = connection.ungrab_key(code, root, ModMask::from(mask));
        }
        let _ = connection.flush();
    }
}
