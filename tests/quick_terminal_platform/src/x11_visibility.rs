use herdr_native_backends_check::{Config, Window, native::Native};
use std::error::Error;
use std::time::{Duration, Instant};
use x11rb::{connection::Connection, protocol::Event, protocol::xproto::*};

pub fn check() -> Result<(), Box<dyn Error>> {
    let (manager, screen) = x11rb::connect(None)?;
    let root = manager.setup().roots[screen].root;
    // A separate parent redirects only this window's map requests.
    let parent = manager.generate_id()?;
    let child = manager.generate_id()?;
    for (id, owner, attributes) in [
        (
            parent,
            root,
            CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(EventMask::SUBSTRUCTURE_REDIRECT),
        ),
        (child, parent, CreateWindowAux::new()),
    ] {
        manager
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                owner,
                0,
                0,
                100,
                100,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &attributes,
            )?
            .check()?;
    }
    manager.map_window(parent)?.check()?;
    let config = Config {
        shortcut: "ctrl-alt-backquote".into(),
        width: 0.5,
        height: 0.5,
        auto_hide: false,
    };
    let mut native = Native::new(&Window(std::num::NonZeroU32::new(child)), &config)?;
    std::thread::scope(|scope| -> Result<(), Box<dyn Error>> {
        scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(Event::MapRequest(event)) = manager.poll_for_event().unwrap()
                    && event.window == child
                {
                    std::thread::sleep(Duration::from_millis(30));
                    manager.map_window(child).unwrap().check().unwrap();
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "no MapRequest for the test window"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let mut results = Vec::new();
        for _ in 0..2 {
            if native.is_visible() {
                native.hide()?;
            } else {
                native.show()?;
            }
            results.push(native.is_visible());
        }
        assert_eq!(
            results,
            [true, false],
            "back-to-back toggles after a delayed map"
        );
        Ok(())
    })?;
    assert!(!native.is_visible(), "the second toggle hides the window");
    let started = Instant::now();
    let error = native.show().expect_err("unanswered MapRequest must fail");
    assert!(error.contains("window manager did not show"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!native.is_visible());
    manager.map_window(child)?.check()?;
    assert!(native.is_visible(), "external mapping is reflected");
    manager.unmap_window(child)?.check()?;
    assert!(!native.is_visible(), "external unmapping is reflected");
    Ok(())
}
