# Quick terminal

Quick-terminal mode keeps Herdr one shortcut away while you work in another application. Press the shortcut to show its existing spaces and tabs, then press it again to hide the window. Hiding leaves terminals and agents running.

## Enable it

Open Settings > Quick terminal and enable the mode. Click the Global shortcut field, then press a key combination or, on macOS and Windows, quickly tap the same modifier twice. The field guides you through the first and second tap. Release all keys to save automatically. Escape, losing focus or closing Settings cancels incomplete input without changing the saved shortcut. Click Restart Herdr to apply changes to the shortcut, initial size or mode. Your tabs and terminals keep running. The button is unavailable while saving a setting or recording a shortcut. The mode is off by default. To return to an ordinary window, turn it off and click Restart Herdr.

The Restart Herdr button appears in the theme's accent color only when saved mode, shortcut or initial size settings differ from startup. It stays visible when you reopen Settings and disappears after restarting or reverting those changes. `status.restart_required` reports the same condition.

Turn off **Hide when switching apps** to keep Herdr above other windows while you work. This change takes effect immediately. The global shortcut still shows and hides the panel.

The defaults are double Cmd on macOS and Ctrl+Alt+backquote on Windows and X11. To use **Left Alt ×2** on Windows, click **Global shortcut**, tap and release left Alt twice, then click **Restart Herdr**. This saves `double-left-alt`. You can also record left or right Ctrl, Shift or Win (`super` in the saved shortcut). On Mac, the corresponding keys are Option, Ctrl, Shift and Cmd. A shortcut without a side, such as `double-alt`, accepts either side. Both taps must use the same side. Release each press within 300 ms and start the second press within 400 ms of the first release. Ordinary chords such as `ctrl-alt-space` accept modifiers on either side. On macOS, the binding follows the physical key when you switch keyboard layouts. Settings shows that key in the current layout. The saved name uses its US ANSI position.

You can use a function key by itself: F1-F20 on macOS or F1-F24 on Windows and X11, except F12, which Windows reserves. Other keys need Ctrl, Alt or Cmd/Super; Shift alone is insufficient. Choose a shortcut that another application does not already use; for example, disable iTerm2's double-Cmd hotkey before assigning the same gesture to Herdr.

On macOS, double-modifier gestures need Accessibility permission for Herdr. In System Settings > Privacy & Security, this is called Device Control and Data Access on macOS 27, or Accessibility on earlier versions. With a double modifier selected, **Enable quick terminal** stays off and disabled until Herdr has permission. Your saved preference is preserved. If the mode was enabled before access was lost, **Use regular window** clears that preference. Choose **Open System Settings** and allow **Herdr.app**, or record an ordinary shortcut such as Ctrl+Option+Space. Opening System Settings hides the panel. When macOS confirms access with System Settings in front, Herdr returns to Quick terminal settings and offers a restart if needed. If Herdr.app is already allowed but access is still unconfirmed, click Herdr in the Dock and choose **Restart and recheck**.

## Settings

The same preferences are stored in `settings.json` under the app home, normally `~/.config/herdr-app`:

```json
{
  "quick_terminal": {
    "enabled": true,
    "shortcut": "ctrl-alt-backquote",
    "width": 1.0,
    "height": 0.6,
    "auto_hide": true
  }
}
```

| Setting | Default | Meaning |
|---|---|---|
| `enabled` | `false` | Use the existing Herdr window as a quick terminal. |
| `shortcut` | `double-cmd` on macOS; `ctrl-alt-backquote` elsewhere | System-wide show/hide shortcut. |
| `width` | `1.0` | Initial fraction of the screen's usable width, from `0.1` to `1.0`. Changing it resets the saved manual size after a restart. |
| `height` | `0.6` | Initial fraction of the screen's usable height, from `0.1` to `1.0`. Changing it resets the saved manual size after a restart. |
| `auto_hide` | `true` | Hide when focus leaves Herdr. Set to `false` to keep the panel visible above other windows. Changes apply immediately. |

`auto_hide` applies to the running window; the other quick-terminal settings take effect on the next start. The shortcut is independent of `keys.json`: its `global` section means all of Herdr's interface, not other applications.

The window appears at the top of the screen under the pointer. Its contents stay in place between calls. Closing or minimizing the window hides it in quick mode; quitting exits the application. If startup fails, for example because another app owns the shortcut, Herdr opens the ordinary window in its saved position and shows the error in Quick terminal settings. Herdr preserves the saved ordinary-window position and size throughout this fallback session. In Settings, choose a different shortcut or select **Use regular window**.

## Resize the window

Drag an edge or corner with the mouse to resize the panel. On macOS and Windows, it stays horizontally centered with equal side margins and its top edge pinned to the screen's usable area throughout the drag. On X11, the window manager controls the drag; Herdr restores the centered position and top alignment as soon as you release the mouse, avoiding jitter during resizing. Herdr remembers the size when you hide and show the window and after restarting the app. Maximize is unavailable in quick mode; resize with the mouse or change the initial dimensions in Settings instead.

The initial `width` and `height` percentages apply until you resize manually. Changing either percentage and restarting Herdr discards the previous manual override and applies the new dimensions. On a smaller display, the panel fits within its usable area while keeping the preferred size for a larger display.

The app saves the manual size in `quick-terminal-window.json` in its home folder, normally `~/.config/herdr-app`. This state is separate from the ordinary window's position and is tied to the operating system and configured `width` and `height` values.

## Platform behavior

| Platform | Behavior |
|---|---|
| macOS | A native panel appears above ordinary and full-screen applications. Double-modifier gestures distinguish left and right and need Accessibility / Device Control and Data Access permission; ordinary shortcuts use system hotkey registration. |
| Windows | A key combination or a double modifier tap shows and hides the topmost window, including while another app has focus. |
| Linux X11 | A key combination or a function key shows and hides a managed window. Placement, activation and stacking depend on the window manager. If a layout change introduces a shortcut conflict, Herdr shows the error and retries until the shortcut becomes available. |
| Linux Wayland | Herdr opens the ordinary window and shows the unsupported session in Settings. |

On Windows, individual modifier presses still reach the active app. Alt may activate its menu and Win may open Start. If Start interrupts recording, set `quick_terminal.shortcut` in `settings.json` (for example, `double-left-super` for Left Win ×2), then restart Herdr. On layouts where right Alt acts as AltGr (Ctrl+Alt), use left Alt for an Alt double tap.

## Commands and troubleshooting

The app must already be running with quick-terminal mode enabled:

```sh
herdr-app do quick_terminal.toggle
herdr-app do quick_terminal.show
herdr-app do quick_terminal.hide
herdr-app status
```

The three commands take no arguments. They return an error when the mode is disabled or could not initialize. `status` includes a `quick_terminal` object with `enabled`, `visible`, the effective `auto_hide`, `error` and `warning`. Check `error` for unsupported-platform errors and shortcut conflicts. On macOS, `warning` identifies missing Accessibility / Device Control and Data Access permission without disabling commands that show and hide the window.

On X11, `show` and `toggle` confirm that the window manager displayed the panel before returning `visible: true`. If the window manager leaves the panel hidden, the command returns an error.

`quick_terminal.permissions` takes no arguments and opens the macOS Accessibility permission page. It also works before quick-terminal mode has been restarted into effect. It never grants permission by itself. `status.quick_terminal.permission` and `window().quick_terminal_permission` report `required`, `granted`, `restart_required` and `recheck_available`. The status command checks access on demand. The window snapshot refreshes while a double-modifier shortcut is active, while its Settings page is open, or during a permission request. After a visit to System Settings, `recheck_available` allows a manual restart while macOS still reports missing access, including before the mode is first enabled. Permission is required if either the running or saved enabled shortcut uses a double modifier. A monitor that started without access, or lost it later, requires a restart after access is restored. The permission warning appears once per missing-access episode when Herdr has focus; the Settings notice remains until access is granted. Other platforms report no permission requirement and reject the permission-page command.

On macOS, `quick_terminal.native` gives the actual panel state: `visible`, `key`, `on_active_space`, `application_active`, `application_hidden`, `auto_hide`, `window_level`, `hides_on_deactivate`, `frame` and `pending_visibility`. While a show/hide is queued, top-level `visible` includes that requested state; `native.visible` reports the window itself. `native.shortcut_activations` counts recognized global shortcut gestures or chords since startup; CLI show/hide commands do not increment it. Compare the counter before and after pressing the physical shortcut to distinguish an input problem from a window-display problem. A nonactivating panel can have `key: true` while `application_active` is false. The `native` field is null on other platforms.

If commands work but a double tap does not, check the modifier side, release the key between taps, and check for conflicting shortcuts in other apps. On macOS, also check Accessibility / Device Control and Data Access permission. If a shortcut cannot be registered, choose another one and restart Herdr. A native Wayland session cannot be fixed by rebinding a key in `keys.json`.

[Back to Herdr](../README.md)
