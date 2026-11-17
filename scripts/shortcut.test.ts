import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

class Element {
  nodes: any[] = [];
  calls: Record<string, any[]> = {};
  constructor() {
    const proxy = new Proxy(this, {
      get: (target, key: string) => key in target ? (target as any)[key] : (...args: any[]) => {
        if (key === "child" || key === "content") target.nodes.push(args[0]);
        else if (key === "children") target.nodes.push(...args[0]);
        else if (key === "when") { if (args[0]) args[1](proxy); }
        else target.calls[key] = args;
        return proxy;
      },
    });
    return proxy;
  }
}

const source = await readFile(new URL("../defaults/frame/settings/main.js", import.meta.url), "utf8");
const element = () => new Element();
let raw: string;
let writes = 0;
let closed = 0;
let focused: any;
let platform = "macos";
let restartRequired: boolean | undefined = false;
let publishedQuick: QuickSettings | undefined;
let runtimeError: string | null = null;
let permission: { required: boolean; granted: boolean; restart_required: boolean; recheck_available?: boolean } | undefined;
let capture: any = { state: "idle" };
let validationError: string | null = null;
let writeError: string | null = null;
let runError: string | null = null;
let actions: any[][] = [];
let validations: (string | undefined)[] = [];
let commands: string[] = [];
let delayed: { ms: number; handler: (cx: any) => void }[] = [];
let tasks: Promise<any>[] = [];
const timers = new Map<number, (cx: any) => void>();
const t = { background: "background", foreground: "foreground", border: "border", raised: "raised", accent: "accent", accent_foreground: "accent-foreground", dim: "dim", blocked: "blocked", working: "working" };
const values: Record<string, any> = {
  View: class {}, div: element, svg: (asset: string) => (element() as any).asset(asset), image: element, v_flex: element, h_flex: element,
  Checkbox: { new: (id: string) => (element() as any).id(id) },
  Button: { new: (id: string) => (element() as any).id(id) },
  Scrollbar: { vertical: element }, InputState: { new: () => ({ on() {} }) },
  Popup: { new: (_id: string, trigger: Element) => (element() as any).child(trigger) },
  props: () => ({ section: "quick-terminal", overlay: "settings-test" }),
  window: () => ({ platform, restart_required: restartRequired, quick_terminal_config: publishedQuick, quick_terminal_error: runtimeError, quick_terminal_permission: permission }),
  theme: () => t, updates: () => ({ app: {}, core: {} }), appHome: () => "/home",
  readTextFile: async () => raw,
  writeTextFile: async (_path: string, next: string, expected: string) => {
    writes++;
    if (writeError) throw new Error(writeError);
    if (expected !== raw) throw new Error("changed since read");
    raw = next;
  },
  closeOverlay() { closed++; },
  run(command: string) {
    commands.push(command);
    if (runError) throw new Error(runError);
  },
  quickTerminalShortcut(action?: string, shortcut?: string) {
    if (action && action !== "validate") actions.push([action, shortcut]);
    if (action === "start") capture = { state: "recording" };
    if (action === "cancel") capture = { state: "cancelled" };
    if (action === "validate") {
      validations.push(shortcut);
      return validationError ? { state: "error", error: validationError } : { state: "valid", shortcut, key_label: keyLabels.get(shortcut ?? "") };
    }
    return capture;
  },
};
const names = [...source.matchAll(/^import \{([^}]*)\}/gm)].flatMap(m => m[1].split(",").map(n => n.trim()));
const keyLabels = new Map<string, string>();
const Settings = new Function(...names, source.replace(/^import .*;\n/gm, "").replace("export default class Settings", "return class Settings"))(...names.map(name => values[name] ?? (() => undefined)));
const cx = {
  notify() {}, stop_propagation() {},
  focus_handle() {
    const handle = { focus() { focused = handle; }, is_focused() { return focused === handle; } };
    return handle;
  },
  timer: {
    every(ms: number, handler: (cx: any) => void) { timers.set(ms, handler); },
    after(ms: number, handler: (cx: any) => void) { delayed.push({ ms, handler }); },
  },
  spawn(handler: (cx: any) => Promise<any>) { tasks.push(handler(cx)); },
};
async function settle() { await Promise.all(tasks.splice(0)); }
type QuickSettings = { enabled?: boolean; shortcut?: string; height?: number };
const doubleOption = { enabled: true, shortcut: "double-left-alt" };

async function view(os = "macos", quick: QuickSettings | null = { enabled: true, shortcut: "ctrl-alt-space", height: 0.6 }) {
  platform = os;
  restartRequired = false;
  publishedQuick = undefined;
  runtimeError = null;
  permission = undefined;
  raw = JSON.stringify({ font_size: 15, quick_terminal: quick ?? undefined });
  writes = closed = 0;
  actions = [];
  validations = [];
  commands = [];
  delayed = [];
  tasks = [];
  validationError = writeError = runError = null;
  capture = { state: "idle" };
  const result = new Settings();
  result.init({}, cx);
  await settle();
  return result;
}
function find(root: any, id: string): Element | undefined {
  if (root?.calls?.id?.[0] === id) return root;
  for (const child of root?.nodes ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
}
function text(root: any): string {
  return typeof root === "string" ? root : (root?.nodes ?? []).map(text).join(" ");
}
function click(v: any, id: string) {
  const target = find(v.render(cx), id);
  expect(target).toBeDefined();
  activate(target!);
}
function activate(target: Element) {
  if (target.calls.on_change) target.calls.on_change[0](!target.calls.checked[0], cx);
  else target.calls.on_click[0]({}, cx);
}
function record(v: any, shortcut: string, ready = true) {
  click(v, "st-shortcut-field");
  capture = { state: "recorded", shortcut, ready };
  timers.get(100)!(cx);
}

test("Quick terminal and Keyboard use distinct shipped sidebar icons", async () => {
  const v = await view();
  const nav = v.render(cx);
  for (const [section, icon] of [["quick-terminal", "terminal"], ["keyboard", "keyboard"]]) {
    const row = find(nav, `st-s-${section}`)!;
    const path = row.nodes.find(node => node.calls.asset)?.calls.asset[0];
    expect(path?.split("/").at(-1)).toBe(`${icon}.svg`);
    expect(await readFile(new URL(`../defaults/${path}`, import.meta.url), "utf8")).toContain("<svg");
  }
});

test("the Mac shortcut shows the current layout label for the same physical binding", async () => {
  const v = await view("macos", { enabled: true, shortcut: "ctrl-q" });
  try {
    for (const cap of ["q", "a", "й"]) {
      keyLabels.set("ctrl-q", cap);
      const label = text(find(v.render(cx), "st-shortcut-field"));
      expect(label).toContain(cap === "й" ? cap : cap.toUpperCase());
    }
    expect(JSON.parse(raw).quick_terminal.shortcut).toBe("ctrl-q");
    expect(writes).toBe(0);
  } finally {
    keyLabels.clear();
  }
});

test("missing Accessibility access names the app and permission without opening settings automatically", async () => {
  const v = await view("macos", doubleOption);
  permission = { required: true, granted: false, restart_required: false };
  const rendered = v.render(cx);
  const notice = find(rendered, "st-quick-permission-notice");
  expect(notice).toBeDefined();
  expect(text(notice)).toContain("Herdr.app");
  expect(text(notice)).toContain("other apps");
  expect(text(notice)).toContain("Device Control and Data Access");
  expect(text(notice)).toContain("Accessibility");
  expect(find(rendered, "st-quick-restart")).toBeUndefined();
  v.render(cx);
  expect(commands).toEqual([]);
  expect(writes).toBe(0);
  click(v, "st-quick-permissions");
  expect(commands).toEqual(["quick_terminal.permissions"]);
  expect(writes).toBe(0);
});

test("an external edit updates the displayed shortcut and permission gate while Settings stays open", async () => {
  const v = await view("macos", { enabled: false, shortcut: "ctrl-alt-space" });
  permission = { required: false, granted: false, restart_required: false };
  const oldSwitch = find(v.render(cx), "st-quick-enable")!;
  const changed = { enabled: false, shortcut: "double-left-alt" };
  raw = JSON.stringify({ quick_terminal: changed });
  activate(oldSwitch);
  await settle();
  expect(writes).toBe(0);
  expect(JSON.parse(raw).quick_terminal).toEqual(changed);
  publishedQuick = changed;
  const tree = v.render(cx);
  expect(find(tree, "st-quick-permissions")).toBeDefined();
  expect(find(tree, "st-quick-enable")!.calls.disabled).toEqual([true]);
  expect(text(find(tree, "st-shortcut-field"))).toContain("Left Option");
  publishedQuick = { enabled: false, shortcut: "ctrl-shift-p" };
  expect(find(v.render(cx), "st-quick-permissions")).toBeUndefined();
  expect(text(find(v.render(cx), "st-shortcut-field"))).toContain("P");
});

test("revoked access allows turning off the saved mode and applying the ordinary window", async () => {
  const v = await view("macos", doubleOption);
  permission = { required: true, granted: false, restart_required: false, recheck_available: true };
  click(v, "st-quick-disable");
  await settle();
  expect(JSON.parse(raw).quick_terminal).toEqual({ ...doubleOption, enabled: false });
  restartRequired = true;
  expect(find(v.render(cx), "st-quick-disable")).toBeUndefined();
  expect(text(find(v.render(cx), "st-quick-restart"))).toBe("Restart Herdr");
  expect(commands).toEqual([]);
});

test("a native initialization error leaves recovery actions available with the switch off", async () => {
  const v = await view("linux");
  runtimeError = "Quick terminal is unavailable on this display";
  const tree = v.render(cx);
  expect(text(find(tree, "st-quick-runtime-error"))).toBe(runtimeError);
  expect(find(tree, "st-quick-enable")!.calls.checked).toEqual([false]);
  expect(find(tree, "st-quick-enable")!.calls.disabled).toEqual([true]);
  expect(find(tree, "st-shortcut-field")).toBeDefined();
  click(v, "st-quick-disable");
  await settle();
  expect(JSON.parse(raw).quick_terminal.enabled).toBe(false);
  expect(find(v.render(cx), "st-quick-runtime-error")).toBeUndefined();
});

test("permission notices are absent for ordinary shortcuts, trusted apps, older hosts and other platforms", async () => {
  for (const os of ["macos", "linux", "windows"]) {
    const v = await view(os);
    for (const state of [undefined, { required: false, granted: false, restart_required: false }, { required: true, granted: true, restart_required: false }]) {
      permission = state;
      expect(find(v.render(cx), "st-quick-permission-notice")).toBeUndefined();
      expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
    }
    if (os !== "macos") {
      v.person.quick_terminal.shortcut = "double-left-alt";
      permission = { required: true, granted: false, restart_required: true };
      expect(find(v.render(cx), "st-quick-permission-notice")).toBeUndefined();
      expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
      expect(find(v.render(cx), "st-quick-enable")!.calls.disabled).toEqual([false]);
    }
    expect(commands).toEqual([]);
  }
});

test("granting and revoking access updates the notice and restart action from the live host", async () => {
  const v = await view("macos", doubleOption);
  permission = { required: true, granted: false, restart_required: false };
  const oldPermissionButton = find(v.render(cx), "st-quick-permissions")!;
  permission = { required: true, granted: true, restart_required: true };
  expect(find(v.render(cx), "st-quick-permission-notice")).toBeUndefined();
  expect(text(v.render(cx))).toContain("Permission granted");
  expect(find(v.render(cx), "st-quick-restart")).toBeDefined();
  oldPermissionButton.calls.on_click[0]({}, cx);
  expect(commands).toEqual([]);
  click(v, "st-quick-restart");
  expect(commands).toEqual(["app.restart"]);
  delayed[0].handler(cx);
  permission = { required: true, granted: true, restart_required: false };
  expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
  expect(text(v.render(cx))).not.toContain("Permission granted");
  permission = { required: true, granted: false, restart_required: false };
  expect(find(v.render(cx), "st-quick-permissions")).toBeDefined();
  expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
  expect(commands).toEqual(["app.restart"]);
});

test("access changes unlock the saved switch choice without writing settings", async () => {
  for (const quick of [null, { enabled: false, shortcut: "double-cmd" }, doubleOption]) {
    const v = await view("macos", quick);
    const enabled = Boolean(quick?.enabled);
    const saved = raw;
    expect(find(v.render(cx), "st-quick-enable")!.calls.disabled).toEqual([false]);
    permission = { required: enabled, granted: false, restart_required: false };
    const denied = v.render(cx);
    const blocked = find(denied, "st-quick-enable")!;
    expect(blocked.calls.disabled).toEqual([true]);
    expect(blocked.calls.checked).toEqual([false]);
    expect(blocked.calls.on_change).toBeUndefined();
    expect(find(denied, "st-shortcut-field")).toBeDefined();
    click(v, "st-quick-permissions");
    expect(commands).toEqual(["quick_terminal.permissions"]);
    permission = { required: enabled, granted: true, restart_required: enabled };
    const granted = v.render(cx);
    const unlocked = find(granted, "st-quick-enable")!;
    expect(unlocked.calls.disabled).toEqual([false]);
    expect(unlocked.calls.checked).toEqual([enabled]);
    expect(Boolean(find(granted, "st-quick-restart"))).toBe(enabled);
    permission.granted = false;
    permission.restart_required = false;
    activate(unlocked);
    await settle();
    expect(writes).toBe(0);
    expect(raw).toBe(saved);
    permission.granted = true;
    click(v, "st-quick-enable");
    await settle();
    expect(writes).toBe(1);
    expect(JSON.parse(raw).quick_terminal.enabled).toBe(!enabled);
  }
});

test("recording an ordinary chord unlocks the switch while the running shortcut still needs access", async () => {
  const v = await view("macos", doubleOption);
  permission = { required: true, granted: false, restart_required: false, recheck_available: true };
  const oldPermission = find(v.render(cx), "st-quick-permissions")!;
  record(v, "ctrl-alt-space");
  await settle();
  restartRequired = true;
  const rendered = v.render(cx);
  expect(find(rendered, "st-quick-enable")!.calls.disabled).toEqual([false]);
  expect(find(rendered, "st-quick-enable")!.calls.checked).toEqual([true]);
  expect(find(rendered, "st-quick-permissions")).toBeUndefined();
  expect(text(find(rendered, "st-quick-restart"))).toBe("Restart Herdr");
  oldPermission.calls.on_click[0]({}, cx);
  expect(commands).toEqual([]);
  expect(JSON.parse(raw).quick_terminal).toEqual({ enabled: true, shortcut: "ctrl-alt-space" });
  expect(writes).toBe(1);
});

test("after a permission visit the user can restart to recheck unconfirmed access", async () => {
  for (const pendingSettings of [false, true]) {
    const v = await view("macos", doubleOption);
    restartRequired = pendingSettings;
    permission = { required: true, granted: false, restart_required: false, recheck_available: true };
    const rendered = text(v.render(cx));
    expect(rendered.match(/Restart and recheck/g)).toHaveLength(1);
    expect(rendered).not.toContain("Apply saved changes");
    expect(rendered).not.toContain("Permission granted");
    expect(commands).toEqual([]);
    click(v, "st-shortcut-field");
    click(v, "st-quick-restart");
    expect(commands).toEqual([]);
    v.cancelShortcut();
    click(v, "st-quick-restart");
    expect(commands).toEqual(["app.restart"]);
  }
});

test("a completed grant invalidates a stale recheck action when restart is unnecessary", async () => {
  const v = await view("macos", doubleOption);
  permission = { required: true, granted: false, restart_required: false, recheck_available: true };
  const staleButton = find(v.render(cx), "st-quick-restart")!;
  permission = { required: true, granted: true, restart_required: false, recheck_available: false };
  staleButton.calls.on_click[0]({}, cx);
  expect(commands).toEqual([]);
  expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
});

test("opening system permissions cancels incomplete shortcut capture and restores ordinary focus", async () => {
  const v = await view("macos", doubleOption);
  permission = { required: true, granted: false, restart_required: false };
  click(v, "st-shortcut-field");
  click(v, "st-quick-permissions");
  expect(actions).toEqual([["start", undefined], ["cancel", undefined]]);
  expect(v.shortcutEdit).toBeNull();
  expect(focused).toBe(v.handle);
  expect(commands).toEqual(["quick_terminal.permissions"]);
  timers.get(100)!(cx);
  expect(actions).toHaveLength(2);
  expect(writes).toBe(0);
});

test("a permission click preserves a completed recording and rechecks access after saving", async () => {
  for (const grantedWhileSaving of [false, true]) {
    const v = await view("macos", doubleOption);
    permission = { required: true, granted: false, restart_required: false };
    click(v, "st-shortcut-field");
    capture = { state: "recorded", shortcut: "double-right-shift", ready: true };
    click(v, "st-quick-permissions");
    expect(v.shortcutSaving).toBe(true);
    expect(commands).toEqual([]);
    if (grantedWhileSaving) permission = { required: true, granted: true, restart_required: true };
    await settle();
    expect(JSON.parse(raw).quick_terminal.shortcut).toBe("double-right-shift");
    expect(writes).toBe(1);
    expect(commands).toEqual(grantedWhileSaving ? [] : ["quick_terminal.permissions"]);
  }
});

test("permission settings failures stay visible and saving blocks an accidental settings launch", async () => {
  const v = await view("macos", doubleOption);
  permission = { required: true, granted: false, restart_required: false };
  v.pendingWrites = 1;
  expect(find(v.render(cx), "st-quick-permissions")!.calls.disabled).toEqual([true]);
  click(v, "st-quick-permissions");
  expect(commands).toEqual([]);
  v.pendingWrites = 0;
  runError = "System Settings could not be opened";
  click(v, "st-quick-permissions");
  expect(text(v.render(cx))).toContain(runError);
  runError = null;
  click(v, "st-quick-permissions");
  expect(v.permissionError).toBeNull();
});

test("switching to an ordinary chord cancels a pending permission visit before the host publishes the change", async () => {
  const v = await view("macos", doubleOption);
  permission = { required: true, granted: false, restart_required: false };
  publishedQuick = doubleOption;
  click(v, "st-shortcut-field");
  capture = { state: "recorded", shortcut: "ctrl-alt-space", ready: true };
  click(v, "st-quick-permissions");
  await settle();
  expect(JSON.parse(raw).quick_terminal.shortcut).toBe("ctrl-alt-space");
  expect(commands).toEqual([]);
});

test("turning off hide on focus loss preserves the shortcut and other preferences", async () => {
  const v = await view();
  click(v, "st-quick-blur");
  await settle();
  expect(writes).toBe(1);
  expect(JSON.parse(raw)).toEqual({
    font_size: 15,
    quick_terminal: { enabled: true, shortcut: "ctrl-alt-space", height: 0.6, auto_hide: false },
  });
  expect(v.person.quick_terminal.auto_hide).toBe(false);
  expect(actions).toEqual([]);
  expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
});

test("restart stays hidden until the host publishes saved pending changes", async () => {
  const v = await view();
  expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
  expect(text(v.render(cx))).not.toContain("Apply saved changes");
  restartRequired = undefined;
  expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
  restartRequired = false;
  click(v, "st-quick-enable");
  expect(v.pendingWrites).toBe(1);
  expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
  await settle();
  expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
  restartRequired = true;
  expect(find(v.render(cx), "st-quick-restart")).toBeDefined();
});

test("published pending changes survive reopening and reverting rejects a stale restart click", async () => {
  await view();
  restartRequired = true;
  const reopened = new Settings();
  reopened.init({}, cx);
  await settle();
  const button = find(reopened.render(cx), "st-quick-restart");
  expect(button).toBeDefined();
  restartRequired = false;
  expect(find(reopened.render(cx), "st-quick-restart")).toBeUndefined();
  button!.calls.on_click[0]({}, cx);
  expect(commands).toEqual([]);
  expect(reopened.restarting).toBe(false);
});

test("held or failed shortcut input never reveals a restart button", async () => {
  for (const fail of [false, true]) {
    const v = await view();
    record(v, "double-left-alt", false);
    expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
    if (fail) writeError = "read-only file system";
    capture = { ...capture, ready: true, phase: "complete" };
    timers.get(100)!(cx);
    expect(v.shortcutSaving).toBe(true);
    expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
    await settle();
    expect(find(v.render(cx), "st-quick-restart")).toBeUndefined();
    expect(commands).toEqual([]);
    if (!fail) {
      restartRequired = true;
      expect(find(v.render(cx), "st-quick-restart")).toBeDefined();
    }
  }
});

test("Restart Herdr appears in the accent color and dispatches once", async () => {
  const v = await view();
  restartRequired = true;
  const button = find(v.render(cx), "st-quick-restart")!;
  expect(button.calls.disabled).toEqual([false]);
  expect(button.calls.bg).toEqual([t.accent]);
  expect(button.calls.border_color).toEqual([t.accent]);
  expect(button.calls.text_color).toEqual([t.accent_foreground]);
  click(v, "st-quick-restart");
  click(v, "st-quick-restart");
  expect(commands).toEqual(["app.restart"]);
  expect(writes).toBe(0);
  expect(text(find(v.render(cx), "st-quick-restart"))).toBe("Restarting…");
  expect(find(v.render(cx), "st-quick-restart")!.calls.disabled).toEqual([true]);
  expect(delayed).toHaveLength(1);
  expect(delayed[0].ms).toBe(3000);
  delayed[0].handler(cx);
  expect(text(find(v.render(cx), "st-quick-restart"))).toBe("Restart Herdr");
  expect(find(v.render(cx), "st-quick-restart")!.calls.disabled).toEqual([false]);
  expect(commands).toEqual(["app.restart"]);
});

test("restart stays disabled until every settings write finishes and failures never restart", async () => {
  for (const fail of [false, true]) {
    const v = await view();
    restartRequired = true;
    if (fail) writeError = "read-only file system";
    click(v, "st-quick-enable");
    click(v, "st-quick-height-plus");
    expect(v.pendingWrites).toBe(2);
    expect(text(find(v.render(cx), "st-quick-restart"))).toBe("Saving…");
    expect(find(v.render(cx), "st-quick-restart")!.calls.disabled).toEqual([true]);
    click(v, "st-quick-restart");
    expect(commands).toEqual([]);
    await settle();
    expect(v.pendingWrites).toBe(0);
    expect(commands).toEqual([]);
    expect(find(v.render(cx), "st-quick-restart")!.calls.disabled).toEqual([false]);
    if (!fail) {
      expect(JSON.parse(raw).quick_terminal).toEqual({ enabled: false, shortcut: "ctrl-alt-space", height: 0.7 });
      click(v, "st-quick-restart");
      expect(commands).toEqual(["app.restart"]);
    }
  }
});

test("restart waits for shortcut capture, release and saving without discarding input", async () => {
  const v = await view();
  restartRequired = true;
  click(v, "st-shortcut-field");
  click(v, "st-quick-restart");
  expect(commands).toEqual([]);
  expect(v.shortcutEdit.state).toBe("recording");
  expect(actions.map(a => a[0])).toEqual(["start"]);
  capture = { state: "recorded", shortcut: "double-right-alt", ready: false, phase: "releasing" };
  timers.get(100)!(cx);
  click(v, "st-quick-restart");
  expect(commands).toEqual([]);
  expect(writes).toBe(0);
  capture = { ...capture, ready: true, phase: "complete" };
  timers.get(100)!(cx);
  expect(text(find(v.render(cx), "st-quick-restart"))).toBe("Saving…");
  click(v, "st-quick-restart");
  expect(commands).toEqual([]);
  await settle();
  expect(JSON.parse(raw).quick_terminal.shortcut).toBe("double-right-alt");
  expect(actions.map(a => a[0])).toEqual(["start", "cancel"]);
  click(v, "st-quick-restart");
  expect(commands).toEqual(["app.restart"]);
});

test("a failed shortcut save does not run a blocked restart request", async () => {
  const v = await view();
  restartRequired = true;
  writeError = "read-only file system";
  record(v, "double-left-alt");
  click(v, "st-quick-restart");
  await settle();
  expect(commands).toEqual([]);
  expect(JSON.parse(raw).quick_terminal.shortcut).toBe("ctrl-alt-space");
  expect(text(v.render(cx))).toContain(writeError);
});

test("restart stays blocked while an invalid shortcut is still held", async () => {
  const v = await view();
  restartRequired = true;
  click(v, "st-shortcut-field");
  capture = { state: "error", error: "Use a modifier with this key", ready: false, phase: "releasing" };
  timers.get(100)!(cx);
  click(v, "st-quick-restart");
  expect(commands).toEqual([]);
  expect(actions.map(a => a[0])).toEqual(["start"]);
  capture = { ...capture, ready: true, phase: "idle" };
  timers.get(100)!(cx);
  expect(find(v.render(cx), "st-quick-restart")!.calls.disabled).toEqual([false]);
  expect(actions.map(a => a[0])).toEqual(["start", "cancel"]);
});

test("a restart dispatch error remains visible and permits retry", async () => {
  const v = await view();
  restartRequired = true;
  runError = "Restart is unavailable";
  click(v, "st-quick-restart");
  expect(commands).toEqual(["app.restart"]);
  expect(text(v.render(cx))).toContain(runError);
  expect(find(v.render(cx), "st-quick-restart")!.calls.disabled).toEqual([false]);
  expect(delayed).toHaveLength(0);
  runError = null;
  click(v, "st-quick-restart");
  expect(commands).toEqual(["app.restart", "app.restart"]);
  expect(text(v.render(cx))).not.toContain("Restart is unavailable");
});

test("shortcut field displays the actual configured chord and sided double taps", async () => {
  const v = await view();
  const field = find(v.render(cx), "st-shortcut-field");
  expect(text(field)).toContain("⌃ ⌥ Space");
  expect(text(field)).toContain("Click to change");
  expect(find(v.render(cx), "st-quick-chord")).toBeUndefined();
  for (const id of ["st-shortcut-use", "st-shortcut-cancel", "st-shortcut-double"]) expect(find(v.render(cx), id)).toBeUndefined();
  expect(text(find(v.render(cx), "st-shortcut-editor"))).not.toContain("Accessibility");
  timers.get(100)!(cx);
  expect(writes).toBe(0);
  for (const [shortcut, label] of [["double-left-cmd", "Left Cmd ×2"], ["double-right-cmd", "Right Cmd ×2"], ["double-cmd", "Cmd ×2"], ["double-right-alt", "Right Option ×2"]]) {
    v.person.quick_terminal.shortcut = shortcut;
    expect(text(find(v.render(cx), "st-shortcut-field"))).toContain(label);
  }
  const portable = await view("windows");
  expect(text(find(portable.render(cx), "st-shortcut-field"))).toContain("Ctrl Alt Space");
  expect(text(find(portable.render(cx), "st-shortcut-editor"))).not.toContain("double-tap");
});

test("released valid recording saves automatically once and preserves other settings", async () => {
  const v = await view();
  record(v, "double-right-cmd", false);
  expect(writes).toBe(0);
  expect(v.person.quick_terminal.shortcut).toBe("ctrl-alt-space");
  expect(text(find(v.render(cx), "st-shortcut-field"))).toContain("Right Cmd ×2");
  expect(text(find(v.render(cx), "st-shortcut-field"))).toContain("Release keys to save");
  capture = { state: "recorded", shortcut: "double-right-cmd" };
  timers.get(100)!(cx);
  expect(writes).toBe(0);
  capture.ready = true;
  timers.get(100)!(cx);
  timers.get(100)!(cx);
  await settle();
  timers.get(100)!(cx);
  expect(writes).toBe(1);
  expect(JSON.parse(raw)).toEqual({ font_size: 15, quick_terminal: { enabled: true, shortcut: "double-right-cmd", height: 0.6 } });
  expect(validations).toContain("double-right-cmd");
  expect(actions.map(a => a[0])).toEqual(["start", "cancel"]);
  expect(v.shortcutEdit).toBeNull();
  expect(text(find(v.render(cx), "st-shortcut-field"))).toContain("Saved · restart required");
  v.shortcutNoticeUntil = Date.now() - 1;
  timers.get(100)!(cx);
  expect(text(find(v.render(cx), "st-shortcut-field"))).toContain("Click to change");
});

for (const [modifier, name] of [["cmd", "Cmd"], ["ctrl", "Ctrl"], ["alt", "Option"], ["shift", "Shift"]]) {
  for (const side of ["left", "right"]) {
    const shortcut = `double-${side}-${modifier}`;
    const label = `${side === "left" ? "Left" : "Right"} ${name}`;
    test(`${shortcut} previews its side and saves once after release`, async () => {
      const v = await view();
      click(v, "st-shortcut-field");
      capture = { state: "recording", preview: `${side}-${modifier}`, ready: false, phase: "modifier-down" };
      timers.get(100)!(cx);
      let field = text(find(v.render(cx), "st-shortcut-field"));
      expect(field).toContain(label);
      expect(field).toContain("Release, then tap again");
      expect(writes).toBe(0);

      capture = { ...capture, phase: "awaiting-second-tap" };
      timers.get(100)!(cx);
      field = text(find(v.render(cx), "st-shortcut-field"));
      expect(field).toContain(label);
      expect(field).toContain("Press again for ×2");

      capture = { state: "recorded", shortcut, ready: false, phase: "releasing" };
      timers.get(100)!(cx);
      field = text(find(v.render(cx), "st-shortcut-field"));
      expect(field).toContain(`${label} ×2`);
      expect(field).toContain("Release keys to save");
      expect(writes).toBe(0);
      expect(JSON.parse(raw).quick_terminal.shortcut).toBe("ctrl-alt-space");

      capture = { ...capture, ready: true, phase: "complete" };
      timers.get(100)!(cx);
      timers.get(100)!(cx);
      await settle();
      timers.get(100)!(cx);
      expect(writes).toBe(1);
      expect(JSON.parse(raw).quick_terminal.shortcut).toBe(shortcut);
      expect(validations).toContain(shortcut);
      expect(actions).toEqual([["start", undefined], ["cancel", undefined]]);
      expect(v.shortcutEdit).toBeNull();
      field = text(find(v.render(cx), "st-shortcut-field"));
      expect(field).toContain(`${label} ×2`);
      expect(field).toContain("Saved · restart required");
    });
  }
}

for (const os of ["macos", "windows", "linux"]) {
  test(`${os} ordinary modifier chords use generic labels and save once after release`, async () => {
    const mac = os === "macos";
    const cases = [
      ["ctrl-space", mac ? "⌃ Space" : "Ctrl Space"],
      ["alt-space", mac ? "⌥ Space" : "Alt Space"],
      ["shift-f8", mac ? "⇧ F8" : "Shift F8"],
      ["super-space", mac ? "⌘ Space" : "Super Space"],
      ["ctrl-alt-shift-super-space", mac ? "⌃ ⌥ ⇧ ⌘ Space" : "Ctrl Alt Shift Super Space"],
      ...(mac ? [["cmd-space", "⌘ Space"], ["ctrl-alt-shift-cmd-space", "⌃ ⌥ ⇧ ⌘ Space"]] : []),
    ];
    for (const [shortcut, label] of cases) {
      const v = await view(os);
      record(v, shortcut, false);
      let field = text(find(v.render(cx), "st-shortcut-field"));
      expect(field).toContain(label);
      expect(field).not.toMatch(/Left|Right|×2/);
      expect(field).toContain("Release keys to save");
      expect(writes).toBe(0);
      expect(JSON.parse(raw).quick_terminal.shortcut).toBe("ctrl-alt-space");

      capture = { ...capture, ready: true, phase: "complete" };
      timers.get(100)!(cx);
      timers.get(100)!(cx);
      await settle();
      timers.get(100)!(cx);
      expect(writes).toBe(1);
      expect(JSON.parse(raw).quick_terminal.shortcut).toBe(shortcut);
      expect(validations).toContain(shortcut);
      expect(actions).toEqual([["start", undefined], ["cancel", undefined]]);
      field = text(find(v.render(cx), "st-shortcut-field"));
      expect(field).toContain(label);
      expect(field).not.toMatch(/Left|Right|×2/);
      expect(field).toContain("Saved · restart required");
    }
  });
}

test("validation failures keep the current shortcut and remain visible during polling", async () => {
  const v = await view("windows");
  validationError = "F12 is reserved by Windows.";
  record(v, "f12");
  timers.get(100)!(cx);
  expect(writes).toBe(0);
  expect(v.person.quick_terminal.shortcut).toBe("ctrl-alt-space");
  expect(text(v.render(cx))).toContain(validationError);
});

test("failed writes report the error without changing the binding or repeatedly retrying", async () => {
  const v = await view();
  writeError = "read-only file system";
  record(v, "ctrl-shift-space");
  await settle();
  timers.get(100)!(cx);
  expect(writes).toBe(3);
  expect(JSON.parse(raw).quick_terminal.shortcut).toBe("ctrl-alt-space");
  expect(v.shortcutEdit.state).toBe("error");
  expect(text(find(v.render(cx), "st-shortcut-field"))).toContain("⌃ ⌥ Space");
  expect(text(v.render(cx))).toContain(writeError);
});

test("native Escape, focus loss and clicking outside discard incomplete captures", async () => {
  for (const action of ["escape", "blur", "outside"]) {
    const v = await view();
    click(v, "st-shortcut-field");
    capture = { state: "recording", preview: "left-shift", phase: "awaiting-second-tap", ready: false };
    timers.get(100)!(cx);
    if (action === "escape") { capture = { state: "cancelled" }; timers.get(100)!(cx); }
    if (action === "blur") { focused = null; timers.get(100)!(cx); }
    if (action === "outside") find(v.render(cx), "st-shortcut-editor")!.calls.on_mouse_down_out[0]({}, cx);
    expect(v.shortcutEdit).toBeNull();
    expect(actions.at(-1)?.[0]).toBe("cancel");
    expect(writes).toBe(0);
    expect(JSON.parse(raw).quick_terminal.shortcut).toBe("ctrl-alt-space");
  }
});

test("leaving the section or closing Settings releases incomplete capture", async () => {
  for (const action of ["section", "close"]) {
    const v = await view();
    click(v, "st-shortcut-field");
    if (action === "section") click(v, "st-s-alerts");
    else v.close(cx);
    expect(v.shortcutEdit).toBeNull();
    expect(actions.map(a => a[0])).toEqual(["start", "cancel"]);
    expect(writes).toBe(0);
    expect(closed).toBe(action === "close" ? 1 : 0);
  }
});

test("one field explains recording and both phases of a sided double tap inline", async () => {
  const v = await view();
  click(v, "st-shortcut-field");
  expect(text(find(v.render(cx), "st-shortcut-field"))).toContain("Press a shortcut or tap one modifier twice");
  expect(text(find(v.render(cx), "st-shortcut-field"))).toContain("Esc");
  capture = { state: "recording", preview: "right-cmd", phase: "modifier-down", ready: false };
  timers.get(100)!(cx);
  let field = find(v.render(cx), "st-shortcut-field");
  expect(text(field)).toContain("Right Cmd");
  expect(text(field)).toContain("Release, then tap again");
  capture.phase = "awaiting-second-tap";
  capture = { ...capture };
  timers.get(100)!(cx);
  field = find(v.render(cx), "st-shortcut-field");
  expect(text(field)).toContain("Press again for ×2");
  for (const id of ["st-shortcut-use", "st-shortcut-cancel", "st-shortcut-double"]) expect(find(v.render(cx), id)).toBeUndefined();
  expect(writes).toBe(0);
});

test("click-out, blur, section change and close save an already released result before leaving", async () => {
  for (const action of ["outside", "blur", "section", "close"]) {
    const v = await view();
    click(v, "st-shortcut-field");
    capture = { state: "recorded", shortcut: "double-right-cmd", ready: true };
    if (action === "outside") find(v.render(cx), "st-shortcut-editor")!.calls.on_mouse_down_out[0]({}, cx);
    if (action === "blur") { focused = null; timers.get(100)!(cx); }
    if (action === "section") click(v, "st-s-alerts");
    if (action === "close") v.close(cx);
    await settle();
    expect(writes).toBe(1);
    expect(JSON.parse(raw).quick_terminal.shortcut).toBe("double-right-cmd");
    expect(v.shortcutEdit).toBeNull();
    expect(closed).toBe(action === "close" ? 1 : 0);
    expect(v.section).toBe(action === "section" ? "alerts" : "quick-terminal");
  }
});

test("a navigation click during saving waits for that one write", async () => {
  const v = await view();
  record(v, "double-left-cmd");
  expect(v.shortcutSaving).toBe(true);
  click(v, "st-s-alerts");
  expect(v.section).toBe("quick-terminal");
  await settle();
  expect(writes).toBe(1);
  expect(v.section).toBe("alerts");
  expect(JSON.parse(raw).quick_terminal.shortcut).toBe("double-left-cmd");
});

test("Escape during saving keeps the pending write and completion notice", async () => {
  for (const action of ["key", "overlay"]) {
    const v = await view();
    record(v, "double-right-cmd");
    expect(v.shortcutSaving).toBe(true);
    const edit = v.shortcutEdit;
    const root = v.render(cx);
    if (action === "key") root.calls.on_key_down[0]({ key: "escape" }, cx);
    else root.calls.on_action[1]({}, cx);
    expect(v.shortcutEdit).toBe(edit);
    expect(actions.map(a => a[0])).toEqual(["start"]);
    await settle();
    timers.get(100)!(cx);
    expect(writes).toBe(1);
    expect(JSON.parse(raw).quick_terminal.shortcut).toBe("double-right-cmd");
    expect(v.shortcutEdit).toBeNull();
    expect(text(find(v.render(cx), "st-shortcut-field"))).toContain("Saved · restart required");
    expect(closed).toBe(0);
  }
});

test("focus starts capture and reusing the current shortcut saves without an extra UI action", async () => {
  const v = await view();
  v.shortcutHandle.focus();
  timers.get(100)!(cx);
  timers.get(100)!(cx);
  expect(actions.map(a => a[0])).toEqual(["start"]);
  capture = { state: "recorded", shortcut: "ctrl-alt-space", ready: true };
  timers.get(100)!(cx);
  await settle();
  expect(writes).toBe(1);
  expect(JSON.parse(raw).quick_terminal.shortcut).toBe("ctrl-alt-space");
  expect(actions.map(a => a[0])).toEqual(["start", "cancel"]);
});

test("an invalid held input keeps capture until release and never saves", async () => {
  const v = await view();
  click(v, "st-shortcut-field");
  capture = { state: "error", error: "Use a modifier with this key", ready: false, phase: "releasing" };
  timers.get(100)!(cx);
  expect(actions.map(a => a[0])).toEqual(["start"]);
  expect(text(find(v.render(cx), "st-shortcut-field"))).toContain("Release keys to continue");
  capture = { ...capture, ready: true, phase: "idle" };
  timers.get(100)!(cx);
  timers.get(100)!(cx);
  expect(actions.map(a => a[0])).toEqual(["start", "cancel"]);
  expect(writes).toBe(0);
  expect(JSON.parse(raw).quick_terminal.shortcut).toBe("ctrl-alt-space");
  expect(text(v.render(cx))).toContain("Use a modifier with this key");
});
