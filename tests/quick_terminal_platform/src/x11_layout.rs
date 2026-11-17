//! A live layout change moves the passive grab and releases stale or partially acquired bindings.

use herdr_native_backends_check::{
    Config, Window,
    mods::{ERRORS, SHORTCUT_ERROR, TOGGLES},
    native::Native,
};
use std::{
    error::Error,
    process::Command,
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{
        xkb::{self, ConnectionExt as _},
        xproto::*,
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

pub fn check(
    connection: &RustConnection,
    root: u32,
    window: &Window,
) -> Result<(), Box<dyn Error>> {
    fn set_layout(layout: &str) {
        assert!(
            Command::new("setxkbmap")
                .args(["-layout", layout])
                .status()
                .unwrap()
                .success()
        );
    }
    let keycode = |symbol| {
        let keys = connection.setup();
        let map = connection
            .get_keyboard_mapping(keys.min_keycode, keys.max_keycode - keys.min_keycode + 1)
            .unwrap()
            .reply()
            .unwrap();
        keys.min_keycode
            + map
                .keysyms
                .chunks(usize::from(map.keysyms_per_keycode))
                .position(|row| row[0] == symbol)
                .unwrap() as u8
    };
    set_layout("us");
    let us_y = keycode(u32::from(b'y'));
    let us_z = keycode(u32::from(b'z'));
    let ctrl = keycode(0xffe3);
    let alt = keycode(0xffe9);
    let native = Native::new(
        window,
        &Config {
            shortcut: "ctrl-alt-y".into(),
            width: 1.0,
            height: 0.6,
            auto_hide: false,
        },
    )?;
    let fire = |code| -> Result<(), Box<dyn Error>> {
        for key in [ctrl, alt, code] {
            connection
                .xtest_fake_input(KEY_PRESS_EVENT, key, 0, root, 0, 0, 0)?
                .check()?;
        }
        for key in [code, alt, ctrl] {
            connection
                .xtest_fake_input(KEY_RELEASE_EVENT, key, 0, root, 0, 0, 0)?
                .check()?;
        }
        connection.flush()?;
        thread::sleep(Duration::from_millis(80));
        Ok(())
    };
    let start = TOGGLES.load(Ordering::SeqCst);
    fire(us_y)?;
    assert_eq!(TOGGLES.load(Ordering::SeqCst), start + 1);
    let block = ModMask::CONTROL | ModMask::M1 | ModMask::LOCK;
    connection
        .grab_key(false, root, block, us_z, GrabMode::ASYNC, GrabMode::ASYNC)?
        .check()?;
    set_layout("de");
    assert_eq!(keycode(u32::from(b'y')), us_z);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ERRORS
        .lock()
        .unwrap()
        .iter()
        .any(|error| error.contains("cannot register"))
    {
        assert!(
            Instant::now() < deadline,
            "missing conflict notification after layout change"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(SHORTCUT_ERROR.lock().unwrap().is_some());
    fire(us_y)?;
    fire(us_z)?;
    assert_eq!(
        TOGGLES.load(Ordering::SeqCst),
        start + 1,
        "old and partial grabs must be released on conflict"
    );
    connection.ungrab_key(us_z, root, block)?.check()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while TOGGLES.load(Ordering::SeqCst) == start + 1 {
        assert!(
            Instant::now() < deadline,
            "shortcut failed to recover after conflict cleared"
        );
        fire(us_z)?;
    }

    assert!(
        SHORTCUT_ERROR.lock().unwrap().is_none(),
        "recovery clears the runtime error"
    );
    set_layout("us,de");
    thread::sleep(Duration::from_millis(200));
    let count = TOGGLES.load(Ordering::SeqCst);
    fire(us_y)?;
    assert_eq!(TOGGLES.load(Ordering::SeqCst), count + 1);
    connection.xkb_use_extension(1, 0)?.reply()?;
    connection
        .xkb_latch_lock_state(
            xkb::ID::USE_CORE_KBD.into(),
            0u8.into(),
            0u8.into(),
            true,
            xkb::Group::M2,
            0u8.into(),
            false,
            0,
        )?
        .check()?;
    thread::sleep(Duration::from_millis(200));
    fire(us_y)?;
    assert_eq!(TOGGLES.load(Ordering::SeqCst), count + 1);
    fire(us_z)?;
    assert_eq!(
        TOGGLES.load(Ordering::SeqCst),
        count + 2,
        "the current XKB group determines the binding"
    );
    // Move Alt from Mod1 to Mod3. The same named modifier still works at its new mask.
    let modifier_map = connection.get_modifier_mapping()?.reply()?;
    let n = usize::from(modifier_map.keycodes_per_modifier());
    let mut codes = modifier_map.keycodes;
    assert!(
        codes[3 * n..4 * n].contains(&alt) && codes[5 * n..6 * n].iter().all(|&code| code == 0)
    );
    for i in 0..n {
        codes.swap(3 * n + i, 5 * n + i);
    }
    assert_eq!(
        connection.set_modifier_mapping(&codes)?.reply()?.status,
        MappingStatus::SUCCESS
    );
    thread::sleep(Duration::from_millis(200));
    fire(us_z)?;
    assert_eq!(
        TOGGLES.load(Ordering::SeqCst),
        count + 3,
        "Alt follows its new modifier mask"
    );
    drop(native);
    Ok(())
}
