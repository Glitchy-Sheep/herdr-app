//! A private hidden HWND exercises global registration and subclass messages without moving focus.

use herdr_native_backends_check::{Config, Window, native::Native};
use std::{
    error::Error,
    num::NonZeroIsize,
    ptr, thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
    UI::{
        Input::KeyboardAndMouse::{
            MOD_ALT, MOD_CONTROL, MOD_SHIFT, RegisterHotKey, UnregisterHotKey, VK_F24,
        },
        Input::{
            GetRegisteredRawInputDevices, RAWINPUTDEVICE, RIDEV_INPUTSINK, RIDEV_NOLEGACY,
            RIDEV_REMOVE, RegisterRawInputDevices,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, SendMessageW, WM_ENTERSIZEMOVE, WM_SIZING,
            WMSZ_BOTTOMRIGHT, WS_OVERLAPPEDWINDOW,
        },
    },
};

struct HiddenWindow(HWND);

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

fn raw_inputs() -> std::io::Result<Vec<RAWINPUTDEVICE>> {
    let size = size_of::<RAWINPUTDEVICE>() as u32;
    let mut count = 0;
    if unsafe { GetRegisteredRawInputDevices(ptr::null_mut(), &mut count, size) } == u32::MAX {
        return Err(std::io::Error::last_os_error());
    }
    let mut inputs = vec![RAWINPUTDEVICE::default(); count as usize];
    if count != 0 {
        let read = unsafe { GetRegisteredRawInputDevices(inputs.as_mut_ptr(), &mut count, size) };
        if read == u32::MAX {
            return Err(std::io::Error::last_os_error());
        }
        inputs.truncate(read as usize);
    }
    Ok(inputs)
}

fn gesture_input(input: &RAWINPUTDEVICE) -> bool {
    input.usUsagePage == 1 && matches!(input.usUsage, 2 | 6)
}

fn check_double_tap_listener(window: &Window, mut config: Config) -> Result<(), Box<dyn Error>> {
    assert!(!raw_inputs()?.iter().any(gesture_input));
    config.shortcut = "double-left-alt".into();
    for _ in 0..2 {
        let native = Native::new(window, &config)?;
        assert!(!native.is_visible());
        let inputs: Vec<_> = raw_inputs()?.into_iter().filter(gesture_input).collect();
        assert_eq!(
            inputs.len(),
            2,
            "mouse and keyboard are registered before the first tap"
        );
        assert_eq!(
            inputs[0].hwndTarget, inputs[1].hwndTarget,
            "one ordered message queue"
        );
        for input in inputs {
            assert!(!input.hwndTarget.is_null());
            assert_ne!(
                input.dwFlags & RIDEV_INPUTSINK,
                0,
                "listens while Herdr is hidden"
            );
            assert_eq!(
                input.dwFlags & RIDEV_NOLEGACY,
                0,
                "ordinary input still reaches applications"
            );
        }
        drop(native);
        let deadline = Instant::now() + Duration::from_secs(5);
        while raw_inputs()?.iter().any(gesture_input) {
            if Instant::now() >= deadline {
                return Err("double-tap listener remained registered after shutdown".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    let keyboard = RAWINPUTDEVICE {
        usUsagePage: 1,
        usUsage: 6,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: window.0.get() as HWND,
    };
    let size = size_of::<RAWINPUTDEVICE>() as u32;
    if unsafe { RegisterRawInputDevices(&keyboard, 1, size) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    assert!(
        Native::new(window, &config).is_err(),
        "the existing keyboard owner keeps its registration"
    );
    let remaining: Vec<_> = raw_inputs()?.into_iter().filter(gesture_input).collect();
    assert_eq!(remaining.len(), 1, "failed registration releases the mouse");
    assert_eq!(remaining[0].usUsage, keyboard.usUsage);
    assert_eq!(remaining[0].hwndTarget, keyboard.hwndTarget);
    let remove = RAWINPUTDEVICE {
        dwFlags: RIDEV_REMOVE,
        hwndTarget: ptr::null_mut(),
        ..keyboard
    };
    if unsafe { RegisterRawInputDevices(&remove, 1, size) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            ptr::null(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            400,
            240,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        )
    };
    let handle = NonZeroIsize::new(hwnd as isize).ok_or_else(std::io::Error::last_os_error)?;
    let _window = HiddenWindow(hwnd);
    let window = Window(handle);
    let config = Config {
        shortcut: "ctrl-alt-shift-f24".into(),
        width: 0.8,
        height: 0.6,
        auto_hide: true,
    };
    let native = Native::new(&window, &config)?;
    assert!(!native.is_visible());
    assert!(
        !raw_inputs()?.iter().any(gesture_input),
        "a chord needs no raw keyboard or mouse subscription"
    );
    let reserve = || unsafe {
        RegisterHotKey(
            ptr::null_mut(),
            1,
            MOD_CONTROL | MOD_ALT | MOD_SHIFT,
            VK_F24.into(),
        ) != 0
    };
    assert!(!reserve(), "the backend owns its global shortcut");

    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        let work = info.rcWork;
        let width = work.right - work.left;
        let mut proposed = RECT {
            left: work.left,
            top: work.top + 20,
            right: work.right - width / 4,
            bottom: work.top + 200,
        };
        unsafe {
            SendMessageW(hwnd, WM_ENTERSIZEMOVE, 0, 0);
            assert_eq!(
                SendMessageW(
                    hwnd,
                    WM_SIZING,
                    WMSZ_BOTTOMRIGHT as usize,
                    &mut proposed as *mut RECT as isize
                ),
                1
            );
        }
        assert_eq!(proposed.top, work.top);
        assert_eq!(proposed.left - work.left, work.right - proposed.right);
    } else {
        eprintln!("No monitor work area is available on this runner; skipping WM_SIZING.");
    }
    drop(native);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !reserve() {
        if Instant::now() >= deadline {
            return Err("hotkey registration was not released".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    unsafe {
        UnregisterHotKey(ptr::null_mut(), 1);
    }
    check_double_tap_listener(&window, config)?;
    println!(
        "PASS: hidden Win32 window, global chord and double Alt listeners, cleanup and re-registration"
    );
    Ok(())
}
