#[cfg(windows)]
mod windows_smoke;
#[cfg(not(windows))]
mod x11_layout;
#[cfg(not(windows))]
mod x11_visibility;

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_smoke::run()
}

#[cfg(not(windows))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use herdr_native_backends_check::{
        Config, SavedSize, Window,
        mods::{HIDES, TOGGLES},
        native::Native,
    };
    use std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    use x11rb::{
        connection::Connection,
        protocol::{xproto::*, xtest::ConnectionExt as _},
        wrapper::ConnectionExt as _,
    };
    if std::env::var("HERDR_QUICK_TEST_ISOLATED").as_deref() != Ok("1") {
        return Err(
            "Use run-x11.sh: keyboard input is allowed only in its isolated X server".into(),
        );
    }
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let create = |name: &str| -> Result<_, Box<dyn std::error::Error>> {
        let id = conn.generate_id()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            id,
            root,
            120,
            120,
            640,
            400,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new().background_pixel(0x224466),
        )?
        .check()?;
        conn.change_property8(
            PropMode::REPLACE,
            id,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            name.as_bytes(),
        )?
        .check()?;
        conn.map_window(id)?.check()?;
        conn.flush()?;
        Ok(id)
    };
    let wait = |name: &str, predicate: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !predicate() {
            assert!(Instant::now() < deadline, "timed out: {name}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let atom = |name: &str| {
        conn.intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    };
    let prop = |window, name: &str| {
        conn.get_property(false, window, atom(name), AtomEnum::ANY, 0, 1024)
            .unwrap()
            .reply()
            .unwrap()
            .value32()
            .map(|v| v.collect::<Vec<_>>())
            .unwrap_or_default()
    };
    wait("window manager ready", &|| {
        !prop(root, "_NET_SUPPORTED").is_empty()
    });
    x11_visibility::check()?;
    let key_names = herdr_native_backends_check::native::KeyNames::new()?;
    let mut key_drain = herdr_native_backends_check::release::Drain::default();
    key_drain.press(&key_names.normalize("("));
    key_drain.release(&key_names.normalize("9"));
    assert!(
        key_drain.ready(false),
        "releasing Shift before a symbol must drain its physical key"
    );
    key_drain.press(&key_names.normalize("~"));
    key_drain.release(&key_names.normalize("`"));
    assert!(key_drain.ready(false));
    let xid = create("Herdr quick terminal smoke")?;
    let previous = create("Previous application")?;
    wait("previous window focus", &|| {
        prop(root, "_NET_ACTIVE_WINDOW").first() == Some(&previous)
    });
    let window = Window(std::num::NonZeroU32::new(xid));
    let config = Config {
        shortcut: "ctrl-alt-backquote".into(),
        width: 0.8,
        height: 0.6,
        auto_hide: true,
    };
    let mut native = Native::new(&window, &config)?;
    let conflict = Native::new(&window, &config);
    assert!(conflict.is_err(), "duplicate shortcut must fail");
    native.show()?;
    wait("quick window focused", &|| {
        prop(root, "_NET_ACTIVE_WINDOW").first() == Some(&xid)
    });
    wait("quick window above", &|| {
        prop(xid, "_NET_WM_STATE").contains(&atom("_NET_WM_STATE_ABOVE"))
    });
    assert!(native.is_visible());
    let geometry = conn.get_geometry(xid)?.reply()?;
    let root_geometry = conn.get_geometry(root)?.reply()?;
    assert_eq!(
        geometry.width,
        (root_geometry.width as f32 * config.width).round() as u16
    );
    assert_eq!(
        geometry.height,
        (root_geometry.height as f32 * config.height).round() as u16
    );
    assert_eq!(
        native.resized(),
        None,
        "initial placement is not a user size preference"
    );
    let dimensions = || {
        let geometry = conn.get_geometry(xid).unwrap().reply().unwrap();
        (geometry.width, geometry.height)
    };
    conn.configure_window(xid, &ConfigureWindowAux::new().width(920).height(360))?
        .check()?;
    conn.flush()?;
    wait("manual resize", &|| dimensions() == (920, 360));
    let saved = native
        .resized()
        .expect("manual resize must produce a saved size");
    assert_eq!(
        saved,
        SavedSize {
            width: 920.0,
            height: 360.0
        }
    );
    native.hide()?;
    wait("quick window hidden", &|| !native.is_visible());
    wait("previous focus restored", &|| {
        prop(root, "_NET_ACTIVE_WINDOW").first() == Some(&previous)
    });
    let remap_started = Instant::now();
    native.show()?;
    assert!(native.is_visible(), "show must confirm the Openbox remap");
    println!("Openbox show completed in {:?}", remap_started.elapsed());
    wait("quick window refocused", &|| {
        prop(root, "_NET_ACTIVE_WINDOW").first() == Some(&xid)
    });
    wait("manual size retained", &|| dimensions() == (920, 360));
    assert_eq!(native.resized(), None);
    let workareas = prop(root, "_NET_WORKAREA");
    let mut smaller = workareas.clone();
    let desktop = prop(root, "_NET_CURRENT_DESKTOP")[0] as usize * 4;
    smaller[desktop + 2] = 640;
    smaller[desktop + 3] = 300;
    conn.change_property32(
        PropMode::REPLACE,
        root,
        atom("_NET_WORKAREA"),
        AtomEnum::CARDINAL,
        &smaller,
    )?
    .check()?;
    native.show()?;
    wait("size clamped to smaller workarea", &|| {
        dimensions() == (640, 300)
    });
    assert_eq!(
        native.resized(),
        None,
        "clamping must not overwrite the preferred size"
    );
    conn.change_property32(
        PropMode::REPLACE,
        root,
        atom("_NET_WORKAREA"),
        AtomEnum::CARDINAL,
        &workareas,
    )?
    .check()?;
    native.show()?;
    wait("preferred size restored on larger workarea", &|| {
        dimensions() == (920, 360)
    });
    assert_eq!(native.resized(), None);
    conn.change_window_attributes(
        xid,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
    )?
    .check()?;
    x11rb::properties::WmSizeHints {
        min_size: Some((920, 360)),
        max_size: Some((920, 360)),
        ..Default::default()
    }
    .set_normal_hints(&conn, xid)?
    .check()?;
    std::thread::sleep(Duration::from_millis(100));
    while conn.poll_for_event()?.is_some() {}
    native.set_saved_size(Some(SavedSize {
        width: 1100.0,
        height: 500.0,
    }));
    native.show()?;
    wait("window manager acknowledges rejected placement", &|| {
        while let Some(event) = conn.poll_for_event().unwrap() {
            if let x11rb::protocol::Event::ConfigureNotify(event) = event
                && event.window == xid
                && (event.width, event.height) == (920, 360)
            {
                return true;
            }
        }
        false
    });
    assert_eq!(native.resized(), None);
    x11rb::properties::WmSizeHints::new()
        .set_normal_hints(&conn, xid)?
        .check()?;
    std::thread::sleep(Duration::from_millis(100));
    conn.configure_window(xid, &ConfigureWindowAux::new().width(1000).height(400))?
        .check()?;
    conn.flush()?;
    wait("manual resize after rejected placement", &|| {
        dimensions() == (1000, 400)
    });
    assert_eq!(
        native.resized(),
        Some(SavedSize {
            width: 1000.0,
            height: 400.0
        })
    );
    let frame = || {
        let extents = prop(xid, "_NET_FRAME_EXTENTS");
        let origin = conn
            .translate_coordinates(xid, root, 0, 0)
            .unwrap()
            .reply()
            .unwrap();
        let (width, height) = dimensions();
        (
            origin.dst_x as i32 - extents[0] as i32,
            origin.dst_y as i32 - extents[2] as i32,
            width as i32 + extents[0] as i32 + extents[1] as i32,
            height as i32 + extents[2] as i32 + extents[3] as i32,
        )
    };
    let centered = || {
        let (x, y, width, _) = frame();
        x == workareas[desktop] as i32 + (workareas[desktop + 2] as i32 - width) / 2
            && y == workareas[desktop + 1] as i32
    };
    wait("manual resize centered before drag", &centered);
    let (x, y, width, height) = frame();
    let drag_x = (x + width - 3) as i16;
    let drag_y = (y + height - 3) as i16;
    conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, root, drag_x, drag_y, 0)?
        .check()?;
    conn.xtest_fake_input(BUTTON_PRESS_EVENT, 1, 0, root, 0, 0, 0)?
        .check()?;
    conn.flush()?;
    std::thread::sleep(Duration::from_millis(60));
    let mut mouse_size = None;
    for step in 1..=3 {
        conn.xtest_fake_input(
            MOTION_NOTIFY_EVENT,
            0,
            0,
            root,
            drag_x + step * 20,
            drag_y + step * 15,
            0,
        )?
        .check()?;
        conn.flush()?;
        std::thread::sleep(Duration::from_millis(120));
        if let Some(size) = native.resized() {
            mouse_size = Some(size);
        }
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(
            frame().0,
            x,
            "do not shift the grabbed edge while the window manager holds the mouse"
        );
    }
    conn.xtest_fake_input(BUTTON_RELEASE_EVENT, 1, 0, root, 0, 0, 0)?
        .check()?;
    conn.flush()?;
    let mouse_size = mouse_size.expect("actual mouse drag must resize the managed window");
    assert!(mouse_size.width > 1000.0 && mouse_size.height > 400.0);
    wait(
        "mouse resize centered and top pinned after release",
        &centered,
    );
    assert_eq!(
        dimensions(),
        (mouse_size.width as u16, mouse_size.height as u16),
        "recentering must not change the chosen size"
    );
    conn.configure_window(xid, &ConfigureWindowAux::new().width(920).height(360))?
        .check()?;
    conn.flush()?;
    wait("restoring manual size", &|| dimensions() == (920, 360));
    assert_eq!(native.resized(), Some(saved));
    std::thread::sleep(Duration::from_millis(100));
    HIDES.store(0, Ordering::SeqCst);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        ClientMessageEvent::new(32, previous, atom("_NET_ACTIVE_WINDOW"), [2, 0, 0, 0, 0]),
    )?
    .check()?;
    conn.flush()?;
    wait("focus loss auto-hide request", &|| {
        HIDES.load(Ordering::SeqCst) > 0
    });
    native.hide()?;
    native.show()?;
    wait("focus after auto-hide", &|| {
        prop(root, "_NET_ACTIVE_WINDOW").first() == Some(&xid)
    });
    native.set_auto_hide(false);
    let hides_before = HIDES.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        ClientMessageEvent::new(32, previous, atom("_NET_ACTIVE_WINDOW"), [2, 0, 0, 0, 0]),
    )?
    .check()?;
    conn.flush()?;
    wait(
        "other window focused after disabling auto-hide live",
        &|| prop(root, "_NET_ACTIVE_WINDOW").first() == Some(&previous),
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        HIDES.load(Ordering::SeqCst),
        hides_before,
        "live pinning suppresses automatic hide requests"
    );
    assert!(
        native.is_visible(),
        "pinned window stays mapped when another app is focused"
    );
    assert!(prop(xid, "_NET_WM_STATE").contains(&atom("_NET_WM_STATE_ABOVE")));
    assert_eq!(
        prop(root, "_NET_CLIENT_LIST_STACKING").last(),
        Some(&xid),
        "pinned window stays above the newly focused window"
    );
    native.set_auto_hide(true);
    native.show()?;
    wait("focus before re-enabled auto-hide", &|| {
        prop(root, "_NET_ACTIVE_WINDOW").first() == Some(&xid)
    });
    std::thread::sleep(Duration::from_millis(100));
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        ClientMessageEvent::new(32, previous, atom("_NET_ACTIVE_WINDOW"), [2, 0, 0, 0, 0]),
    )?
    .check()?;
    conn.flush()?;
    wait(
        "live auto-hide enabled again without recreating backend",
        &|| HIDES.load(Ordering::SeqCst) > hides_before,
    );
    native.hide()?;
    wait(
        "re-enabled auto-hide hides after its request is dispatched",
        &|| !native.is_visible(),
    );
    native.show()?;
    wait("focus after live auto-hide cycle", &|| {
        prop(root, "_NET_ACTIVE_WINDOW").first() == Some(&xid)
    });
    let setup = conn.setup();
    let mapping = conn
        .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)?
        .reply()?;
    let key = |sym| {
        setup.min_keycode
            + mapping
                .keysyms
                .chunks(mapping.keysyms_per_keycode as usize)
                .position(|keys| keys.contains(&sym))
                .unwrap() as u8
    };
    let ctrl = key(0xffe3);
    let alt = key(0xffe9);
    let grave = key(0x60);
    for code in [ctrl, alt, grave] {
        conn.xtest_fake_input(KEY_PRESS_EVENT, code, 0, root, 0, 0, 0)?
            .check()?;
    }
    conn.flush()?;
    wait("global shortcut", &|| TOGGLES.load(Ordering::SeqCst) == 1);
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(
        TOGGLES.load(Ordering::SeqCst),
        1,
        "held shortcut must not repeat"
    );
    for code in [grave, alt, ctrl] {
        conn.xtest_fake_input(KEY_RELEASE_EVENT, code, 0, root, 0, 0, 0)?
            .check()?;
    }
    conn.flush()?;
    std::thread::sleep(Duration::from_millis(50));
    for code in [ctrl, alt, grave] {
        conn.xtest_fake_input(KEY_PRESS_EVENT, code, 0, root, 0, 0, 0)?
            .check()?;
    }
    conn.flush()?;
    wait("second shortcut", &|| TOGGLES.load(Ordering::SeqCst) == 2);
    for code in [grave, alt, ctrl] {
        conn.xtest_fake_input(KEY_RELEASE_EVENT, code, 0, root, 0, 0, 0)?
            .check()?;
    }
    conn.flush()?;
    drop(native);
    let config = Config {
        auto_hide: false,
        ..config
    };
    let mut native = Native::new(&window, &config)?;
    native.set_saved_size(Some(saved));
    native.show()?;
    wait("focus after re-registering", &|| {
        prop(root, "_NET_ACTIVE_WINDOW").first() == Some(&xid)
    });
    wait("saved size restored after native restart", &|| {
        dimensions() == (920, 360)
    });
    assert_eq!(native.resized(), None);
    let hides_before = HIDES.load(Ordering::SeqCst);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        ClientMessageEvent::new(32, previous, atom("_NET_ACTIVE_WINDOW"), [2, 0, 0, 0, 0]),
    )?
    .check()?;
    conn.flush()?;
    wait("focus with auto-hide disabled", &|| {
        prop(root, "_NET_ACTIVE_WINDOW").first() == Some(&previous)
    });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        HIDES.load(Ordering::SeqCst),
        hides_before,
        "auto-hide must respect the setting"
    );
    assert!(native.is_visible());
    native.set_auto_hide(true);
    native.show()?;
    wait("focus after enabling initially disabled auto-hide", &|| {
        prop(root, "_NET_ACTIVE_WINDOW").first() == Some(&xid)
    });
    std::thread::sleep(Duration::from_millis(100));
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        ClientMessageEvent::new(32, previous, atom("_NET_ACTIVE_WINDOW"), [2, 0, 0, 0, 0]),
    )?
    .check()?;
    conn.flush()?;
    wait("startup-disabled watcher supports enabling live", &|| {
        HIDES.load(Ordering::SeqCst) > hides_before
    });
    native.hide()?;
    wait(
        "startup-disabled backend hides after enabling live",
        &|| !native.is_visible(),
    );
    drop(native);
    let config = Config {
        shortcut: "f8".into(),
        ..config
    };
    let native = Native::new(&window, &config)?;
    conn.xtest_fake_input(KEY_PRESS_EVENT, key(0xffc5), 0, root, 0, 0, 0)?
        .check()?;
    conn.flush()?;
    wait("standalone function key", &|| {
        TOGGLES.load(Ordering::SeqCst) == 3
    });
    conn.xtest_fake_input(KEY_RELEASE_EVENT, key(0xffc5), 0, root, 0, 0, 0)?
        .check()?;
    conn.flush()?;
    use herdr_native_backends_check::quick_terminal::{RECORDED, RECORDING, RECORDINGS};
    RECORDING.store(true, Ordering::SeqCst);
    for recorded in 1..=2 {
        std::thread::sleep(Duration::from_millis(50));
        conn.xtest_fake_input(KEY_PRESS_EVENT, key(0xffc5), 0, root, 0, 0, 0)?
            .check()?;
        conn.flush()?;
        wait("current global shortcut forwarded to recorder", &|| {
            RECORDINGS.load(Ordering::SeqCst) == recorded
        });
        assert_eq!(RECORDED.lock().unwrap().as_deref(), Some("f8"));
        assert_eq!(
            TOGGLES.load(Ordering::SeqCst),
            3,
            "recording and preview must suppress toggling"
        );
        assert!(
            !native.keyboard_released()?,
            "recording cannot finish while its key is held"
        );
        assert!(
            !key_names.keyboard_released()?,
            "a recorder without quick mode must also wait for release"
        );
        conn.xtest_fake_input(KEY_RELEASE_EVENT, key(0xffc5), 0, root, 0, 0, 0)?
            .check()?;
        conn.flush()?;
        wait("recorded key released", &|| {
            native.keyboard_released().unwrap()
        });
    }
    conn.xtest_fake_input(KEY_PRESS_EVENT, ctrl, 0, root, 0, 0, 0)?
        .check()?;
    conn.flush()?;
    assert!(
        !native.keyboard_released()?,
        "remaining modifiers must drain too"
    );
    assert!(!key_names.keyboard_released()?);
    conn.xtest_fake_input(KEY_RELEASE_EVENT, ctrl, 0, root, 0, 0, 0)?
        .check()?;
    conn.flush()?;
    wait("remaining modifier released", &|| {
        native.keyboard_released().unwrap()
    });
    RECORDING.store(false, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(50));
    conn.xtest_fake_input(KEY_PRESS_EVENT, key(0xffc5), 0, root, 0, 0, 0)?
        .check()?;
    conn.flush()?;
    wait("global shortcut restored after recording", &|| {
        TOGGLES.load(Ordering::SeqCst) == 4
    });
    conn.xtest_fake_input(KEY_RELEASE_EVENT, key(0xffc5), 0, root, 0, 0, 0)?
        .check()?;
    conn.flush()?;
    drop(native);
    x11_layout::check(&conn, root, &window)?;
    println!(
        "PASS: X11 layout and group changes, remapping conflicts and recovery, shortcut conflict, show/above/focus, hide/restore, remap/refocus, resize persistence, rejected placement, mouse drag and centering after release, workarea clamping, native restart, live auto-hide and pinned stacking, global keys, standalone function key, recorder forwarding and toggle suppression, no auto-repeat, re-registration"
    );
    Ok(())
}
