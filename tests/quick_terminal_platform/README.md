# Quick terminal platform checks

This opt-in harness includes the real `src/quick_terminal/linux.rs` or `windows.rs` modules by relative path. A small GPUI window-handle adapter and request recorder replace the application shell; backend code is not copied. It is a separate Cargo workspace and does not change the application's dependencies.

Run parser and geometry unit tests on Linux or macOS, without opening a window:

```sh
cargo test --locked --manifest-path tests/quick_terminal_platform/Cargo.toml
```

Check the Windows backend against the actual Win32 bindings, including its tests:

```sh
rustup target add x86_64-pc-windows-gnu
cargo check --locked --tests --target x86_64-pc-windows-gnu --manifest-path tests/quick_terminal_platform/Cargo.toml
```

On Windows, run the native smoke test with `cargo run --locked --manifest-path tests/quick_terminal_platform/Cargo.toml`. It creates a hidden HWND, verifies registration and cleanup of Ctrl+Alt+Shift+F24 and, on runners with a monitor, sends resize messages to its own window. It leaves focus in the current application. Foreground restoration, physical shortcut presses, drag behavior and mixed-DPI positioning also need a desktop session.

Run the X11 integration test with Docker.

```sh
bash tests/quick_terminal_platform/run-x11.sh
```

Linux hosts with Xvfb, Openbox, xauth and setxkbmap installed can run `bash tests/quick_terminal_platform/run-x11.sh --host`. Both entry points create their own X server and cookie through xvfb-run. The Docker entry point mounts the harness and backend read-only. The test injects input only into its isolated display. Do not run its binary directly against a desktop display.

The smoke test covers conflicting shortcut registration, show/hide and remapping, topmost state, window dimensions, native resize observation, preserving a manual size across summoning and backend recreation, rejected window placement, clamping to a smaller workarea without losing the preference, a real Openbox mouse resize, previous-focus restoration, auto-hide enabled and disabled, global keyboard events, a standalone function key, held-key repeat suppression and shortcut cleanup/re-registration, US/German layout changes, active XKB group and modifier-mask changes, and recovery after a conflict during remapping. It checks the native backend; GPUI mouse hit targets, persisted JSON and the full application still need their normal tests. Native Wayland mode is explicitly unsupported and is not covered by this X11 test.

The recorder stub checks that pressing the currently registered global shortcut forwards its configured value while suppressing toggles throughout recording and preview. Toggling resumes after recording ends; the global registration stays reserved.

Live auto-hide checks toggle the same backend from enabled to disabled and back, and enable a backend that started disabled. With auto-hide disabled, focusing another window emits no hide request and keeps the panel mapped above it. Enabling auto-hide again produces the dedicated automatic-hide request on the next blur; the harness dispatches that request through `hide()` and checks that the panel is unmapped.

Recorder drain tests cover overlapping keys, repeats, modifiers, native physical-key probes with and without quick mode, and Shift-symbol normalization when Shift is released first. Completion requires both the queued key releases and physical keyboard state to be clear.

On X11 the window manager owns the mouse resize: the backend waits until the mouse button is released, then centers the final width and pins the top edge. It does not move the grabbed edge during the drag. The mouse smoke test checks both phases. Windows uses `WM_SIZING` to keep the panel symmetric and top-pinned during the drag; the hidden HWND smoke exercises the native subclass alongside the geometry unit tests.
