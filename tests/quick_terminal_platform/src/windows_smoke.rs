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
    println!(
        "PASS: hidden Win32 window, native shortcut registration, cleanup and re-registration"
    );
    Ok(())
}
