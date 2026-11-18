// App-owned settings mod. Changes save immediately; shortcuts wait for key release.

import { View, div, svg, image } from "gpui-kit";
import { v_flex, h_flex, Scrollbar, InputState, Input, Popup, Checkbox, Button } from "gpui-base";
import { hc, run, theme, themes, fonts, updates, telemetry, settings, layouts, shared, appHome, readTextFile, writeTextFile, playSound, props, closeOverlay, notify, window, quickTerminalShortcut } from "herdr";

const I = (name) => `frame/settings/${name}.svg`;

const SECTIONS = [
  { id: "look", title: "Look", icon: I("palette") },
  { id: "quick-terminal", title: "Quick terminal", icon: "frame/welcome/terminal.svg" },
  ...(window()?.platform === "macos" ? [{ id: "keyboard", title: "Keyboard", icon: I("keyboard") }] : []),
  { id: "machines", title: "Machines", icon: I("server") },
  { id: "alerts", title: "Alerts", icon: I("bell") },
  { id: "updates", title: "Updates", icon: I("refresh") },
  { id: "privacy", title: "Privacy", icon: I("shield") },
];

const LOOKS = { main: "Herdr", terminals: "Terminals", win98: "Herdr 98" };
const APPEARANCES = [
  [null, "System"],
  ["light", "Light"],
  ["dark", "Dark"],
];

// What the app uses when settings.json names no font (settings.rs).
const DEFAULT_FONT = "JetBrains Mono";
const DEFAULT_UI_FONT = "Geist";

function shortcutKeys(value, platform) {
  const text = String(value ?? "").trim().toLowerCase();
  const mac = platform === "macos";
  const superKey = mac ? "Cmd" : platform === "windows" ? "Win" : "Super";
  const altKey = mac ? "Option" : "Alt";
  const names = { cmd: superKey, command: superKey, meta: superKey, super: superKey, win: superKey, ctrl: "Ctrl", control: "Ctrl", alt: altKey, opt: altKey, option: altKey, shift: "Shift" };
  const modifier = (name) => names[name] ?? name;
  const double = text.match(/^double-(?:(left|right)-)?(.+)$/);
  if (double) return [`${double[1] ? `${double[1] === "left" ? "Left" : "Right"} ` : ""}${modifier(double[2])}`, "×2"];
  const side = text.match(/^(left|right)-(cmd|ctrl|alt|shift|win|super)$/);
  if (side) return [`${side[1] === "left" ? "Left" : "Right"} ${modifier(side[2])}`];
  const symbols = { Cmd: "⌘", Ctrl: "⌃", Option: "⌥", Shift: "⇧" };
  const keys = { space: "Space", backquote: "`", grave: "`", backtick: "`", escape: "Esc", esc: "Esc", return: mac ? "Return" : "Enter", enter: mac ? "Return" : "Enter", tab: "Tab", backspace: mac ? "⌫" : "Backspace", left: "←", right: "→", up: "↑", down: "↓", minus: "-", equal: "=", bracketleft: "[", bracketright: "]", quote: "'", semicolon: ";", backslash: "\\", comma: ",", period: ".", slash: "/" };
  return text.split(/[+-]/).filter(Boolean).map((part) => {
    const name = names[part];
    if (name) return (mac && symbols[name]) || name;
    return keys[part] ?? (/^(?:[a-z0-9]|f\d+)$/.test(part) ? part.toUpperCase() : part);
  });
}

export default class Settings extends View {
  init(_props, cx) {
    const asked = props()?.section;
    this.section = SECTIONS.some((s) => s.id === asked) ? asked : "look";
    this.picking = null; // "font" | "ui_font" while its list is open
    this.filter = "";
    this.fontInput = InputState.new({ placeholder: "Type to search fonts" });
    this.fontInput.on("change", (_e, cx) => { this.filter = this.fontInput.value(); cx.notify(); });
    this.renameInput = InputState.new();
    this.renameInput.on("focus", (_e, cx) => { this.renameFocused = true; cx.notify(); });
    this.renameInput.on("blur", (_e, cx) => { this.renameFocused = false; cx.notify(); });
    this.renameInput.on("change", (_e, cx) => { if (this.machineEdit?.action === "rename") this.machineEdit.name = this.renameInput.value(); cx.notify(); });
    this.renameInput.on("submit", (_e, cx) => {
      const edit = this.machineEdit;
      if (edit?.action === "rename") this.machineAction(edit.machine, "rename", cx, edit.name);
    });
    this.person = {};
    this.raw = null;
    this.pendingWrites = 0;
    this.restarting = false;
    this.restartError = null;
    this.permissionError = null;
    this.previews = {}; // layout name -> its "preview" picture
    this.previewsOf = "";
    this.showData = false;
    this.machineList = null;
    this.machineLoading = false;
    this.machineError = null;
    this.machineBusy = {};
    this.machineResults = {};
    this.machineEdit = null;
    this.machineNotice = null;
    this.handle = cx.focus_handle();
    this.shortcutHandle = cx.focus_handle();
    this.shortcutFocused = false;
    this.shortcutEdit = null;
    this.shortcutSaving = false;
    this.shortcutAfterSave = null;
    this.shortcutNotice = null;
    this.shortcutNoticeUntil = 0;
    this.handle.focus();
    cx.spawn(async (cx) => {
      await this.load();
      if (this.section === "machines") await this.loadMachines();
      cx.notify();
    });
    // After "Check now": follow the look (the answer comes from the app), then
    // "Up to date" for a moment.
    cx.timer.every(300, (cx) => {
      if (!this.asked) return;
      if (updates().checking) this.asked = Date.now();
      else if (Date.now() - this.asked >= 3000) this.asked = null;
      cx.notify();
    });
    cx.timer.every(3000, (cx) => {
      if (this.section !== "machines") return;
      cx.spawn(async (cx) => {
        await this.loadMachines();
        cx.notify();
      });
    });
    cx.timer.every(100, (cx) => this.pollShortcut(cx));
  }

  file() {
    return `${appHome()}/settings.json`;
  }

  // Each layout file may name a picture of itself (`"preview"` on its root).
  loadPreviews(names, cx) {
    this.previewsOf = names.join("\n");
    cx.spawn(async (cx) => {
      const found = {};
      for (const name of names) {
        try {
          const preview = JSON.parse((await readTextFile(`${appHome()}/layouts/${name}.json`)) ?? "{}").preview;
          if (typeof preview === "string" && preview) found[name] = preview;
        } catch {}
      }
      this.previews = found;
      cx.notify();
    });
  }

  // `broken`: settings.json doesn't parse (mid-edit), or isn't an object, so
  // nothing may be written over it.
  async load() {
    this.raw = await readTextFile(this.file());
    try {
      const person = this.raw ? JSON.parse(this.raw) : {};
      this.broken = !person || typeof person !== "object" || Array.isArray(person);
      this.person = this.broken ? {} : person;
    } catch {
      this.person = {};
      this.broken = true;
    }
  }

  // Changes settings.json the way an agent would: read, change, write back
  // only if nobody changed it meanwhile (else read again and redo). Says so
  // when it couldn't.
  change(edit, cx, allowed) {
    this.pendingWrites = (this.pendingWrites ?? 0) + 1;
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const failed = await this.save(edit, allowed);
        if (failed) notify({ ...failed, level: "error" });
      } finally {
        this.pendingWrites--;
        cx.notify();
      }
    });
  }

  // The toast saying why `edit` wasn't written, or null once it is.
  async save(edit, allowed) {
    let error;
    for (let attempt = 0; attempt < 3; attempt++) {
      let next;
      try {
        await this.load();
        if (this.broken) return { title: "settings.json has errors: fix it first" };
        if (allowed && !allowed(this.person)) return null;
        next = JSON.parse(JSON.stringify(this.person));
        edit(next);
      } catch (e) {
        return { title: "Cannot read settings.json", body: String(e?.message ?? e) };
      }
      try {
        await writeTextFile(this.file(), JSON.stringify(next, null, 2) + "\n", this.raw);
        this.person = next;
        return null;
      } catch (e) {
        error = e;
      }
    }
    return { title: "Not saved to settings.json", body: String(error?.message ?? error) };
  }

  close(cx) {
    if (cx) this.leaveShortcut(cx, () => closeOverlay(props().overlay));
    else { this.cancelShortcut(); closeOverlay(props().overlay); }
  }

  // Pieces ---------------------------------------------------------------

  heading(t, text) {
    return div().pt(6).pb(8).text_size(12).font_semibold().text_color(t.dim).child(text.toUpperCase());
  }

  // One setting: what it is on the left, the control on the right.
  row(t, title, about, control) {
    return h_flex()
      .w_full()
      .gap(16)
      .py(12)
      .items_center()
      .border_b(1)
      .border_color(t.border)
      .child(
        v_flex()
          .flex_1()
          .min_w(0)
          .gap(3)
          .child(div().text_size(14).text_color(t.foreground).child(title))
          .when(Boolean(about), (el) => el.child(div().text_size(12.5).line_height("18px").text_color(t.dim).child(about))),
      )
      .child(div().flex_none().child(control));
  }

  button(t, id, label, onClick, component) {
    const button = component ? component.new(id).accessibility_label(label) : div().id(id);
    return button
      .px(12)
      .py(5)
      .rounded(7)
      .border_1()
      .border_color(t.border)
      .bg(t.raised)
      .cursor_pointer()
      .text_size(13)
      .text_color(t.foreground)
      .hover((s) => s.bg(t.hover))
      .on_click(onClick)
      .child(label);
  }

  // − value +
  stepper(t, id, value, onMinus, onPlus) {
    const b = (k, label, f) =>
      div()
        .id(`${id}-${k}`)
        .w(28)
        .h(28)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_size(15)
        .text_color(t.muted)
        .hover((s) => s.bg(t.hover).text_color(t.foreground))
        .on_click(f)
        .child(label);
    return h_flex()
      .items_center()
      .rounded(7)
      .border_1()
      .border_color(t.border)
      .bg(t.raised)
      .overflow_hidden()
      .child(b("minus", "\u2212", onMinus))
      .child(div().w(54).text_size(13).text_color(t.foreground).flex().justify_center().child(value))
      .child(b("plus", "+", onPlus));
  }

  toggle(t, id, label, on, onChange, disabled = false) {
    return Checkbox.new(id).role("switch").accessibility_label(label)
      .checked(on)
      .disabled(disabled)
      .w(36)
      .h(20)
      .rounded(10)
      .p(2)
      .when(!disabled, (el) => el.cursor_pointer().on_change(onChange))
      .when(disabled, (el) => el.opacity(0.5))
      .bg(on ? t.accent : t.hover)
      .flex()
      .when(on, (el) => el.justify_end())
      .child(div().w(16).h(16).rounded(8).bg(on ? t.accent_foreground : t.muted));
  }

  // Look -----------------------------------------------------------------

  // A small drawing of each layout, in the theme's colors (as the welcome draws them).
  preview(t, name) {
    const row = () => div().flex().flex_row();
    const col = () => div().flex().flex_col();
    const term = (bg = t.background, edge = t.border) => div().flex_1().rounded(2).bg(bg).border_1().border_color(edge);
    const line = (w, c = t.dim) => div().h(3).w(w).flex_none().rounded(2).bg(c);
    const lines = (n, c) => col().gap(4).p(5).children([...Array(n)].map((_, i) => line(i % 2 ? 22 : 32, c)));
    const frame = (el) => el.size_full().rounded(6).overflow_hidden().border_1().border_color(t.border).bg(t.surface);
    if (name === "main")
      return frame(row())
        .child(col().w(42).border_r(1).border_color(t.border).child(lines(4)))
        .child(
          col()
            .flex_1()
            .child(row().h(11).gap(2).p(2).border_b(1).border_color(t.border).child(div().w(20).rounded(2).bg(t.accent)).child(div().w(16).rounded(2).bg(t.raised)).child(div().w(16).rounded(2).bg(t.raised)))
            .child(row().flex_1().p(4).gap(3).child(term()).child(term())),
        );
    if (name === "terminals") {
      const card = () => col().flex_1().rounded(4).border_1().border_color("#2c2f34").bg("#17191c").child(row().h(7).px(3).items_center().border_b(1).border_color("#25282c").child(line(14, "#5d6168")));
      return frame(col())
        .bg("#131619")
        .p(4)
        .gap(4)
        .child(row().h(10).gap(3).p(1).rounded(4).bg("#1d1f22").child(div().w(30).rounded(3).bg("#34373c").border_1().border_color("#4a4d53")).child(div().w(30)))
        .child(row().flex_1().gap(4).child(card()).child(card()));
    }
    // A layout of the person's own: a plain frame.
    return frame(col()).child(row().h(11).border_b(1).border_color(t.border)).child(row().flex_1().p(4).child(term()));
  }

  layoutPicker(t, cx) {
    const current = shared("layout") ?? "main";
    // Shipped looks in the welcome's order, then the person's own; new files show up by themselves.
    const shipped = Object.keys(LOOKS);
    const rank = (n) => (shipped.includes(n) ? shipped.indexOf(n) : shipped.length);
    const names = [...layouts()].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
    if (names.join("\n") !== this.previewsOf) this.loadPreviews(names, cx);
    return h_flex()
      .w_full()
      .flex_wrap()
      .gap(8)
      .children(
        names.map((name) => {
          const on = name === current;
          return v_flex()
            .id(`st-l-${name}`)
            .w(124)
            .gap(6)
            .p(6)
            .rounded(10)
            .border_2()
            .border_color(on ? t.accent : "#00000000")
            .bg(on ? t.raised : "#00000000")
            .cursor_pointer()
            .hover((s) => s.bg(t.raised))
            .on_click((_e, cx) => {
              run("layout.switch", { name });
              cx.notify();
            })
            .child(
              div()
                .h(62)
                .w_full()
                .overflow_hidden()
                .child(
                  this.previews[name]
                    ? image(this.previews[name]).w_full().h(62).rounded(6).border_1().border_color(t.border)
                    : this.preview(t, name),
                ),
            )
            .child(
              h_flex()
                .gap(5)
                .items_center()
                .child(div().text_size(12.5).text_color(on ? t.foreground : t.muted).child(LOOKS[name] ?? name))
                .when(on, (el) => el.child(svg(I("check")).size(12).text_color(t.accent))),
            );
        }),
      );
  }

  appearancePicker(t, cx) {
    const now = this.person.appearance ?? null;
    return h_flex()
      .p(2)
      .gap(2)
      .rounded(8)
      .border_1()
      .border_color(t.border)
      .bg(t.background)
      .children(
        APPEARANCES.map(([value, label]) =>
          div()
            .id(`st-ap-${label}`)
            .px(12)
            .py(4)
            .rounded(6)
            .text_size(13)
            .cursor_pointer()
            .when(now === value, (el) => el.bg(t.raised).text_color(t.foreground).border_1().border_color(t.border))
            .when(now !== value, (el) => el.text_color(t.muted).hover((s) => s.text_color(t.foreground)))
            .on_click((_e, cx) =>
              this.change((p) => {
                if (value === null) delete p.appearance;
                else p.appearance = value;
              }, cx),
            )
            .child(label),
        ),
      );
  }

  themePicker(t, light) {
    const s = settings();
    const current = light ? s.theme_light : s.theme_dark;
    return h_flex()
      .w_full()
      .flex_wrap()
      .gap(8)
      .children(
        // A theme named like a layout (themes/win98.json) is that layout's own.
        themes()
          .filter((x) => Boolean(x.light) === light && (x.name === shared("layout") || !layouts().includes(x.name)))
          .map(({ name, colors: c }) => {
          const on = name === current;
          const label = name.replace(/-/g, " ");
          const dot = (color) => div().w(8).h(8).rounded(4).bg(color);
          return v_flex()
            .id(`st-t-${name}`)
            .w(124)
            .gap(6)
            .p(6)
            .rounded(10)
            .border_2()
            .border_color(on ? t.accent : "#00000000")
            .cursor_pointer()
            .hover((s) => s.bg(t.raised))
            .on_click((_e, cx) => {
              run("app.theme", { name });
              cx.notify();
            })
            .child(
              v_flex()
                .h(46)
                .w_full()
                .p(7)
                .gap(6)
                .rounded(6)
                .border_1()
                .border_color(t.border)
                .bg(c.background)
                .child(div().h(4).w(44).rounded(2).bg(c.terminal_foreground))
                .child(h_flex().gap(4).child(dot(c.accent)).child(dot(c.ansi1)).child(dot(c.ansi2)).child(dot(c.ansi3)).child(dot(c.ansi5))),
            )
            .child(
              h_flex()
                .w_full()
                .min_w(0)
                .gap(5)
                .items_center()
                .child(div().min_w(0).truncate().tooltip(label).text_size(12.5).text_color(on ? t.foreground : t.muted).child(label))
                .when(on, (el) => el.child(svg(I("check")).size(12).flex_none().text_color(t.accent))),
            );
        }),
      );
  }

  // The font in use, written in itself; a click opens the list.
  fontButton(t, which, family, def) {
    const open = this.picking === which;
    const trigger = div()
      .id(`st-f-${which}`)
      .w(220)
      .px(12)
      .py(6)
      .rounded(7)
      .border_1()
      .border_color(open ? t.accent : t.border)
      .bg(t.raised)
      .cursor_pointer()
      .hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => {
        this.picking = open ? null : which;
        this.filter = "";
        this.fontInput.set_value("");
        if (this.picking) this.fontInput.focus();
        else this.handle.focus();
        cx.notify();
      })
      .child(div().font_family(family).text_size(13.5).text_color(t.foreground).child(family));
    return Popup.new(`st-fp-${which}`, trigger)
      .when(open, (el) => el.content(this.fontList(t, which, def)));
  }

  fontList(t, which, def) {
    const q = this.filter.toLowerCase();
    const matches = fonts().filter((f) => f.toLowerCase().includes(q)).slice(0, 200);
    const choose = (name, cx) => {
      this.change((p) => {
        if (name === null) delete p[which];
        else p[which] = name;
      }, cx);
      this.picking = null;
      this.handle.focus();
      cx.notify();
    };
    const item = (id, label, family, onClick) =>
      div()
        .id(id)
        .px(10)
        .py(5)
        .rounded(6)
        .cursor_pointer()
        .hover((s) => s.bg(t.hover))
        .on_click(onClick)
        .child(div().font_family(family).text_size(13.5).text_color(t.foreground).child(label));
    return v_flex()
      .id(`st-font-list-${which}`)
      .w(360)
      .on_mouse_down_out((_e, cx) => {
        this.picking = null;
        this.handle.focus();
        cx.notify();
      })
      .rounded(8)
      .border_1()
      .border_color(t.border)
      .bg(t.background)
      // Let the child's native scroll run before stopping the page's wheel event.
      .on_scroll_wheel((_e, cx) => cx.stop_propagation())
      .child(
        div()
          .px(12)
          .py(8)
          .border_b(1)
          .border_color(t.border)
          .text_size(13)
          .child(Input.new(this.fontInput).h(28).text_color(t.foreground)),
      )
      .child(
        div()
          .id(`st-fl-${which}`)
          .h(200)
          .p(4)
          .overflow_y_scrollbar()
          .when(!q, (el) => el.child(item(`st-fd-${which}`, `Default (${def})`, def, (_e, cx) => choose(null, cx))))
          .children(matches.map((f, i) => item(`st-fi-${which}-${i}`, f, f, (_e, cx) => choose(f, cx)))),
      );
  }

  look(t, cx) {
    const s = settings();
    const p = this.person;
    // Numbers come back from the host as 32-bit floats: 1.1 reads as 1.100000023841858.
    const same = (a, b) => (typeof a === "number" && typeof b === "number" ? Math.abs(a - b) < 1e-4 : a === b);
    const own = (key, composed, def) => !same(p[key] ?? def, composed);
    const size = (key, value, d, cx) => this.change((q) => (q[key] = Math.max(8, Math.min(32, (q[key] ?? value) + d))), cx);
    const step = (key, value, d, lo, hi, cx) => this.change((q) => (q[key] = Math.round(Math.max(lo, Math.min(hi, (q[key] ?? value) + d)) * 100) / 100), cx);
    const fontRow = (which, sizeKey, title, family, def, sizeNow) =>
      v_flex()
        .w_full()
        .child(
          this.row(
            t,
            title,
            own(which, family, def) ? "This layout sets its own; yours shows in the others." : null,
            h_flex()
              .gap(8)
              .items_center()
              .child(this.fontButton(t, which, family, def))
              // The panels' text follows zoom: they size their rows to it.
              .when(Boolean(sizeKey), (el) => el.child(this.stepper(t, `st-${sizeKey}`, `${Math.round(sizeNow)}`, (_e, cx) => size(sizeKey, sizeNow, -1, cx), (_e, cx) => size(sizeKey, sizeNow, 1, cx)))),
          ),
        );
    return v_flex()
      .w_full()
      .child(this.heading(t, "Layout"))
      .child(this.layoutPicker(t, cx))
      .child(div().h(18))
      .child(this.heading(t, "Theme"))
      .child(this.row(t, "Appearance", "Light or dark, or as your system is now.", this.appearancePicker(t, cx)))
      .child(div().pt(12).pb(8).text_size(12.5).text_color(t.dim).child("For the layout you're in: each layout keeps its own light and dark theme."))
      .child(div().pb(6).text_size(12.5).font_medium().text_color(t.muted).child("Dark"))
      .child(this.themePicker(t, false))
      .child(div().pt(10).pb(6).text_size(12.5).font_medium().text_color(t.muted).child("Light"))
      .child(this.themePicker(t, true))
      .child(div().h(18))
      .child(this.heading(t, "Size and fonts"))
      .child(
        this.row(
          t,
          "Zoom",
          "Everything: terminals, panels, text.",
          h_flex()
            .gap(8)
            .items_center()
            .child(this.stepper(t, "st-zoom", `${Math.round(s.zoom * 100)}%`, () => run("app.zoom_out"), () => run("app.zoom_in")))
            .child(this.button(t, "st-zoom-reset", "Reset", () => run("app.zoom_reset"))),
        ),
      )
      .child(fontRow("font", "font_size", "Terminal font", s.font_family, DEFAULT_FONT, s.font_size))
      .child(
        this.row(
          t,
          "Terminal font weight",
          own("font_weight", s.font_weight, 500) ? "This layout sets its own; yours shows in the others." : "Bold text stays at 700.",
          this.stepper(t, "st-font_weight", `${s.font_weight}`, (_e, cx) => step("font_weight", s.font_weight, -100, 100, 900, cx), (_e, cx) => step("font_weight", s.font_weight, 100, 100, 900, cx)),
        ),
      )
      .children([
        ["terminal_line_height", "Terminal line height", 0.8, 2, 0.1],
        ["terminal_cell_width", "Terminal cell width", 0.8, 2, 0.05],
        ["terminal_scroll_speed", "Terminal scroll speed", 0.1, 5, 0.1],
      ].map(([key, title, min, max, step]) => {
        const by = (d) => (_e, cx) => this.change((q) => (q[key] = Math.max(min, Math.min(max, Math.round(Math.round(((q[key] ?? s[key]) + d) / step) * step * 100) / 100))), cx);
        return this.row(
          t, title,
          own(key, s[key], 1) ? "This layout sets its own; yours shows in the others." : null,
          this.stepper(t, `st-${key}`, `${Math.round(s[key] * 100)}%`, by(-step), by(step)),
        );
      }))
      .child(fontRow("ui_font", null, "Interface font", s.ui_font_family, DEFAULT_UI_FONT));
  }

  keyboard(t) {
    const choices = [
      ["auto", "Automatic (Alt on US keyboards)"],
      ["on", "Alt"],
      ["off", "Types characters"],
      ["left", "Left Option is Alt"],
      ["right", "Right Option is Alt"],
    ];
    return v_flex()
      .w_full()
      .child(this.heading(t, "Option key"))
      .child(div().pb(12).text_size(13).text_color(t.dim).child("Use Option for terminal shortcuts or to type accents and special characters."))
      .children(choices.map(([value, label]) => {
        const on = (this.person.option_as_alt ?? "auto") === value;
        return h_flex()
          .id(`st-option-${value}`)
          .gap(10)
          .px(12)
          .py(10)
          .rounded(7)
          .items_center()
          .cursor_pointer()
          .hover((s) => s.bg(t.hover))
          .on_click((_e, cx) => this.change((p) => { p.option_as_alt = value; }, cx))
          .child(div().text_color(on ? t.accent : t.muted).child(on ? "◉" : "○"))
          .child(div().text_size(14).text_color(t.foreground).child(label));
      }));
  }

  startShortcut(cx) {
    if (this.shortcutSaving) return;
    this.shortcutNotice = null;
    this.shortcutNoticeUntil = 0;
    this.shortcutHandle.focus();
    this.shortcutFocused = true;
    const result = quickTerminalShortcut("start");
    this.shortcutEdit = result?.state === "error" ? result : { state: "recording" };
    cx.notify();
  }

  cancelShortcut(cx, restoreFocus = false) {
    if (this.shortcutSaving) return;
    if (!this.shortcutEdit) return;
    quickTerminalShortcut("cancel");
    this.shortcutEdit = null;
    if (restoreFocus) this.handle.focus();
    cx?.notify();
  }

  leaveShortcut(cx, after) {
    if (this.shortcutSaving) {
      if (after) this.shortcutAfterSave = after;
      return;
    }
    if (this.shortcutEdit && ["recording", "recorded"].includes(this.shortcutEdit.state)) {
      const result = quickTerminalShortcut();
      if (result?.state === "recorded" && result.ready === true) {
        this.saveShortcut(result, cx, after);
        return;
      }
    }
    this.cancelShortcut(cx);
    after?.(cx);
  }

  pollShortcut(cx) {
    if (this.shortcutNotice && Date.now() >= this.shortcutNoticeUntil) {
      this.shortcutNotice = null;
      cx.notify();
    }
    const focused = this.section === "quick-terminal" && this.shortcutHandle.is_focused();
    const gainedFocus = focused && !this.shortcutFocused;
    this.shortcutFocused = focused;
    if (gainedFocus && !this.shortcutEdit && !this.shortcutSaving) {
      this.startShortcut(cx);
      return;
    }
    const edit = this.shortcutEdit;
    if (!edit || this.shortcutSaving || !["recording", "recorded", "error"].includes(edit.state) || (edit.state === "error" && edit.ready !== false)) return;
    const result = quickTerminalShortcut();
    if (result?.state === "recorded" && result.ready === true) {
      this.saveShortcut(result, cx);
      return;
    }
    if (!focused) {
      this.cancelShortcut(cx);
      return;
    }
    if (!result || result.state === "idle") return;
    if (result.state === "cancelled") {
      this.cancelShortcut(cx, true);
    } else if (JSON.stringify(result) !== JSON.stringify(edit)) {
      this.shortcutEdit = result;
      if (result.state === "error" && result.ready !== false) quickTerminalShortcut("cancel");
      cx.notify();
    }
  }

  saveShortcut(edit, cx, after) {
    if (this.shortcutSaving || !edit.shortcut || edit.ready !== true) return;
    const validation = quickTerminalShortcut("validate", edit.shortcut);
    if (validation?.state !== "valid") {
      quickTerminalShortcut("cancel");
      this.shortcutEdit = { state: "error", error: validation?.error ?? "This shortcut is not supported. Try another one." };
      if (after) {
        notify({ title: "Shortcut was not changed", body: this.shortcutEdit.error, level: "error" });
        after(cx);
      }
      cx.notify();
      return;
    }
    this.shortcutEdit = edit;
    this.shortcutSaving = true;
    this.shortcutAfterSave = after ?? null;
    cx.notify();
    cx.spawn(async (cx) => {
      const failed = await this.save((person) => {
        person.quick_terminal = { ...(person.quick_terminal ?? {}), shortcut: edit.shortcut };
      });
      this.shortcutSaving = false;
      const afterSave = this.shortcutAfterSave;
      this.shortcutAfterSave = null;
      if (this.shortcutEdit === edit) {
        if (failed) {
          quickTerminalShortcut("cancel");
          this.shortcutEdit = { state: "error", error: [failed.title, failed.body].filter(Boolean).join(". ") };
        }
        else {
          this.cancelShortcut(cx, true);
          this.shortcutNotice = "Saved · restart required";
          this.shortcutNoticeUntil = Date.now() + 2500;
        }
      }
      if (afterSave) {
        if (failed) notify({ ...failed, level: "error" });
        afterSave(cx, failed ? undefined : this.person.quick_terminal);
      }
      cx.notify();
    });
  }

  shortcutField(t, platform, current) {
    const edit = this.shortcutEdit;
    const recording = edit?.state === "recording";
    const recorded = edit?.state === "recorded";
    const drainingError = edit?.state === "error" && edit.ready === false;
    const value = recorded ? edit.shortcut : recording ? edit.preview : current;
    const labels = shortcutKeys(value, platform);
    if (platform === "macos" && typeof value === "string" && !recording && !/^double-/i.test(value.trim())) {
      const cap = quickTerminalShortcut("validate", value)?.key_label;
      if (cap) labels[labels.length - 1] = /^[a-z]$/i.test(cap) ? cap.toUpperCase() : cap;
    }
    let hint = platform === "macos" || platform === "windows" ? "Press a shortcut or tap one modifier twice" : "Press a shortcut or a function key";
    if (drainingError) hint = "Release keys to continue";
    else if (recorded) hint = "Release keys to save";
    else if (edit?.phase === "modifier-down") hint = "Release, then tap again";
    else if (edit?.phase === "awaiting-second-tap") hint = "Press again for ×2";
    const active = recording || recorded || drainingError;
    const spoken = labels[1] === "×2" ? `${labels[0]} twice` : labels.join(" + ");
    const trailing = this.shortcutSaving ? "Saving…" : active ? "Esc" : this.shortcutNotice ?? "Click to change";
    return v_flex().id("st-shortcut-editor").w_full().gap(8).py(12).border_b(1).border_color(t.border)
      .on_mouse_down_out((_e, cx) => this.leaveShortcut(cx))
      .child(div().text_size(14).text_color(t.foreground).child("Global shortcut"))
      .child(h_flex().id("st-shortcut-field").track_focus(this.shortcutHandle).tab_index(0).role("button")
        .accessibility_label(active ? [spoken, hint, "Escape cancels."].filter(Boolean).join(". ") : `Global shortcut: ${spoken}. Click to change.`)
        .w_full().min_h(44).px(12).py(8).gap(10).items_center().rounded(7).border_1()
        .border_color(active ? t.accent : t.border).bg(t.background).cursor_pointer()
        .hover((s) => s.bg(t.raised))
        .on_click((_e, cx) => this.startShortcut(cx))
        .on_key_down((e, cx) => {
          if (!edit && ["enter", "space"].includes(e.key)) { this.startShortcut(cx); cx.stop_propagation(); }
        })
        .child(h_flex().flex_1().min_w(0).gap(5).items_center()
          .children(labels.map((label) => div().px(7).py(3).rounded(4).border_1().border_color(t.border).bg(t.raised).text_size(13).text_color(t.foreground).child(label)))
          .when(active && !this.shortcutSaving, (el) => el.child(div().min_w(0).ml(labels.length ? 5 : 0).text_size(12).text_color(t.dim).child(hint))))
        .child(div().flex_none().text_size(12).text_color(active ? t.accent : t.dim).child(trailing)))
      .when(Boolean(edit?.error), (el) => el.child(div().text_size(12).line_height("18px").text_color(t.blocked).child(edit.error)));
  }

  restartBlocked() {
    const edit = this.shortcutEdit;
    const capturing = edit && (["recording", "recorded"].includes(edit.state) || edit.ready === false);
    return Boolean(this.restarting || this.pendingWrites || this.shortcutSaving || capturing);
  }

  restartOffer(permission = this.quickTerminalPermission()) {
    const pending = window()?.restart_required;
    if (pending && !this.quickConfig().enabled) permission = null;
    if (permission?.granted && permission.restart_required) {
      return { title: "Permission granted", about: "Restart to use the shortcut in other apps.", label: "Restart Herdr" };
    }
    if (permission?.recheck_available && !permission.granted) {
      return { title: "Access still unconfirmed", about: "If Herdr.app is already allowed, restart to check again.", label: "Restart and recheck" };
    }
    if (pending) {
      return { title: "Apply saved changes", about: "Your tabs and terminals keep running.", label: "Restart Herdr" };
    }
    return null;
  }

  quickConfig() {
    return window()?.quick_terminal_config ?? this.person.quick_terminal ?? {};
  }

  quickTerminalPermission(config = this.quickConfig()) {
    const host = window();
    const permission = host?.quick_terminal_permission;
    if (host?.platform !== "macos" || !permission || (permission.granted && !permission.restart_required)) return null;
    const shortcut = config.shortcut ?? "double-cmd";
    if (typeof shortcut !== "string" || !/^double-/i.test(shortcut.trim())) return null;
    return quickTerminalShortcut("validate", shortcut)?.state === "valid" ? permission : null;
  }

  openQuickTerminalPermissions(cx) {
    if (this.restarting || this.pendingWrites || this.shortcutSaving) return;
    this.leaveShortcut(cx, (_cx, saved) => {
      const permission = this.quickTerminalPermission(saved);
      if (!permission || permission.granted) return;
      this.handle.focus();
      this.permissionError = null;
      try {
        run("quick_terminal.permissions");
      } catch (error) {
        this.permissionError = String(error?.message ?? error);
      }
      cx.notify();
    });
  }

  quickTerminalPermissionNotice(t) {
    const busy = Boolean(this.restarting || this.pendingWrites || this.shortcutSaving);
    return v_flex().id("st-quick-permission-notice").w_full().gap(8).p(12).mt(8).rounded(8).border_1().border_color(t.working).bg(t.raised)
      .child(div().text_size(14).font_semibold().text_color(t.foreground).child("Allow double-tap shortcuts"))
      .child(div().text_size(12.5).line_height("18px").text_color(t.foreground).child("For double taps in other apps, allow Herdr.app under Device Control and Data Access (Accessibility on older macOS versions). Or use a shortcut like Ctrl + Option + Space."))
      .child(h_flex().child(this.button(t, "st-quick-permissions", "Open System Settings", (_e, cx) => this.openQuickTerminalPermissions(cx), Button)
        .disabled(busy).when(busy, (el) => el.opacity(0.5))))
      .when(Boolean(this.permissionError), (el) => el.child(div().text_size(12).line_height("18px").text_color(t.blocked).child(this.permissionError)));
  }

  restartHerdr(cx) {
    if (!this.restartOffer() || this.restartBlocked()) return;
    this.restarting = true;
    this.restartError = null;
    cx.notify();
    try {
      run("app.restart");
      cx.timer.after(3000, (cx) => {
        this.restarting = false;
        cx.notify();
      });
    } catch (error) {
      this.restarting = false;
      this.restartError = String(error?.message ?? error);
      cx.notify();
    }
  }

  quickTerminal(t) {
    const q = this.quickConfig();
    const platform = window()?.platform;
    const shortcut = q.shortcut ?? (platform === "macos" ? "double-cmd" : "ctrl-alt-backquote");
    const set = (key, value, cx, allowed) => this.change((p) => {
      p.quick_terminal = { ...(p.quick_terminal ?? {}), [key]: value };
    }, cx, allowed);
    const restartBlocked = this.restartBlocked();
    const permission = this.quickTerminalPermission();
    const needsPermission = permission?.granted === false;
    const runtimeError = q.enabled && window()?.quick_terminal_error;
    const blocked = Boolean(needsPermission || runtimeError);
    const changeEnabled = (enabled, cx) => {
      if (this.quickTerminalPermission()?.granted === false || (this.quickConfig().enabled && window()?.quick_terminal_error)) return;
      set("enabled", enabled, cx, (p) => !enabled || this.quickTerminalPermission(p.quick_terminal ?? {})?.granted !== false);
    };
    const offer = this.restartOffer(permission);
    const restartLabel = this.restarting ? "Restarting…" : this.pendingWrites || this.shortcutSaving ? "Saving…" : offer?.label;
    return v_flex().w_full()
      .child(this.heading(t, "Quick terminal"))
      .child(this.row(t, "Enable quick terminal", blocked ? null : "Show or hide Herdr from any app. Hidden tabs keep running.",
        h_flex().items_center().gap(10)
          .when(blocked && Boolean(q.enabled), (el) => el.child(this.button(t, "st-quick-disable", "Use regular window", (_e, cx) => set("enabled", false, cx), Button)))
          .child(this.toggle(t, "st-quick-enable", "Enable quick terminal", !blocked && Boolean(q.enabled), changeEnabled, blocked))))
      .when(Boolean(runtimeError), (el) => el.child(div().id("st-quick-runtime-error").py(8).text_size(13).text_color(t.blocked).child(runtimeError)))
      .when(needsPermission, (el) => el.child(this.quickTerminalPermissionNotice(t)))
      .when(Boolean(offer), (el) => el.child(this.row(t, offer.title, this.restartError ?? offer.about,
        this.button(t, "st-quick-restart", restartLabel, (_e, cx) => this.restartHerdr(cx), Button)
          .bg(t.accent).border_color(t.accent).text_color(t.accent_foreground).hover((s) => s.bg(t.accent).opacity(0.9))
          .disabled(restartBlocked).when(restartBlocked, (el) => el.opacity(0.5)))))
      .child(this.shortcutField(t, platform, shortcut))
      .child(this.row(t, "Hide when switching apps", "When off, Herdr stays above other windows.", this.toggle(t, "st-quick-blur", "Hide when switching apps", q.auto_hide ?? true, (enabled, cx) => set("auto_hide", enabled, cx))))
      .child(div().pt(12).child(this.heading(t, "Initial size")))
      .child(div().text_size(12.5).line_height("18px").text_color(t.dim).child("Percent of the usable screen. Changes reset the saved window size on restart."))
      .children([["height", "Height", 0.6], ["width", "Width", 1]].map(([key, label, fallback]) => {
        const value = q[key] ?? fallback;
        const step = (delta) => (_e, cx) => set(key, Math.min(1, Math.max(0.1, Math.round(value * 10 + delta) / 10)), cx);
        return this.row(t, label, null, this.stepper(t, `st-quick-${key}`, `${Math.round(value * 100)}%`, step(-1), step(1)));
      }));
  }

  // Machines -------------------------------------------------------------

  async loadMachines() {
    if (this.machineLoading) return;
    this.machineLoading = true;
    try {
      const r = await hc(["machine", "ls", "--json"]);
      if (r.code !== 0) throw new Error(r.stderr || r.stdout || "Couldn't read machines.");
      const list = JSON.parse(r.stdout).machines;
      if (!Array.isArray(list)) throw new Error("Couldn't read machines.");
      this.machineList = list.filter((m) => !m.temporary);
      this.machineError = null;
    } catch (e) {
      this.machineError = String(e);
    } finally {
      this.machineLoading = false;
    }
  }

  machineAction(machine, action, cx, name) {
    if (this.machineBusy[machine]) return;
    if (action === "rename" && (!name?.trim() || name.trim() === machine)) return;
    this.machineBusy[machine] = action;
    delete this.machineResults[machine];
    this.machineEdit = null;
    this.handle.focus();
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const args = ["machine", action, machine];
        if (action === "rename") args.push(name.trim());
        const r = await hc(args);
        let payload = null;
        try { payload = JSON.parse(r.stdout || r.stderr); } catch {}
        if (r.code !== 0) throw new Error(payload?.error?.message || (r.stderr || r.stdout || "It didn't work.").trim());
        const message = action === "update"
          ? `Updated ${machine}${payload?.version ? ` to hc ${payload.version}` : ""}; its terminals kept running.`
          : action === "rename" ? `Renamed ${machine} to ${name.trim()}.` : `Removed ${machine}; its terminals keep running there.`;
        if (action === "update") this.machineResults[machine] = { message, ok: true };
        else this.machineNotice = message;
      } catch (e) {
        this.machineResults[machine] = { message: String(e), ok: false };
      } finally {
        delete this.machineBusy[machine];
      }
      await this.loadMachines();
      cx.notify();
    });
  }

  olderCore(remote, local) {
    const parse = (v) => /^(\d+)\.(\d+)\.(\d+)(?:-([^+]+))?(?:\+.*)?$/.exec(v ?? "");
    const a = parse(remote), b = parse(local);
    if (!a || !b) return false;
    for (let i = 1; i <= 3; i++) {
      if (+a[i] !== +b[i]) return +a[i] < +b[i];
    }
    if (!a[4] || !b[4]) return Boolean(a[4] && !b[4]);
    const x = a[4].split("."), y = b[4].split(".");
    for (let i = 0; i < Math.max(x.length, y.length); i++) {
      if (x[i] === y[i]) continue;
      if (x[i] === undefined || y[i] === undefined) return x[i] === undefined;
      const xn = /^\d+$/.test(x[i]), yn = /^\d+$/.test(y[i]);
      return xn && yn ? +x[i] < +y[i] : xn !== yn ? xn : x[i] < y[i];
    }
    return false;
  }

  machineRow(t, m, localVersion) {
    const busy = this.machineBusy[m.name];
    const edit = this.machineEdit?.machine === m.name ? this.machineEdit : null;
    const state = String(m.state ?? "unknown").replace(/[_-]/g, " ");
    const color = ["connected", "online"].includes(m.state) ? t.ansi2 : ["connecting", "reconnecting"].includes(m.state) ? t.working : t.blocked;
    const result = this.machineResults[m.name];
    const controls = h_flex().gap(6).items_center();
    if (busy) controls.child(svg("mods/workspace-chrome/spinner.svg").size(12).text_color(t.working).spin(900)).child(div().text_size(12).text_color(t.muted).child(busy === "update" ? "Updating…" : busy === "rename" ? "Renaming…" : "Removing…"));
    else {
      if (this.olderCore(m.version, localVersion)) controls.child(this.button(t, `st-m-update-${m.name}`, "Update", (_e, cx) => this.machineAction(m.name, "update", cx)));
      for (const [action, label] of [["rename", "Rename"], ["rm", "Remove"]]) {
        controls.child(this.button(t, `st-m-${action}-${m.name}`, label, (_e, cx) => {
          this.machineEdit = { machine: m.name, action, name: m.name };
          if (action === "rename") {
            this.renameInput.set_value(m.name);
            this.renameInput.set_selected_range({ start: 0, end: encodeURIComponent(m.name).replace(/%[0-9A-F]{2}/g, "_").length });
            this.renameFocused = true;
            this.renameInput.focus();
          }
          cx.notify();
        }));
      }
    }
    const row = v_flex().w_full().py(12).gap(8).border_b(1).border_color(t.border)
      .child(h_flex().gap(10).items_center()
        .child(div().w(7).h(7).flex_none().rounded(4).bg(color))
        .child(v_flex().flex_1().min_w(0).gap(3)
          .child(div().text_size(14).text_color(t.foreground).child(m.name))
          .child(div().text_size(12.5).text_color(t.dim).child(`${m.target ?? ""} · ${state} · ${m.version ? `hc ${m.version}` : "hc version unknown"}`)))
        .when(m.keeps_running_after_logout === false, el => el.child(div().id(`st-m-logout-${m.name}`)
          .text_color(t.blocked).cursor_pointer().tooltip("Agents stop when you disconnect. Click for the fix.")
          .on_click(() => run("mod.open", { name: "machine-add" })).child("⚠")))
        .child(controls.flex_none()));
    if (m.state === "needs_login") {
      const command = `hc machine login ${m.name}`;
      row.child(v_flex().gap(5)
        .child(div().text_size(12.5).text_color(t.blocked).child("needs a login: run"))
        .child(h_flex().gap(8).items_center()
          .child(div().flex_1().min_w(0).text_size(12.5).text_color(t.foreground).child(command))
          .child(this.button(t, `st-m-login-copy-${m.name}`, this.copiedLogin === command ? "Copied" : "Copy", (_e, cx) => {
            cx.write_to_clipboard(command);
            this.copiedLogin = command;
            cx.notify();
          }))));
    }
    if (busy === "update") row.child(div().text_size(12.5).text_color(t.dim).child("Copying this computer's herdr-core. Its terminals keep running."));
    if (edit && !busy) {
      const cancel = this.button(t, `st-m-cancel-${m.name}`, "Cancel", (_e, cx) => { this.machineEdit = null; this.handle.focus(); cx.notify(); });
      if (edit.action === "rm") row.child(v_flex().gap(8)
        .child(div().text_size(12.5).text_color(t.muted).child(`Remove ${m.name}? Its terminals keep running there.`))
        .child(h_flex().gap(6).child(cancel).child(this.button(t, `st-m-confirm-${m.name}`, "Remove machine", (_e, cx) => this.machineAction(m.name, "rm", cx)))));
      else row.child(v_flex().gap(6)
        .child(div().text_size(12).text_color(t.dim).child("New name (addresses using the old name stop working)"))
        .child(h_flex().gap(6).items_center()
          .child(Input.new(this.renameInput).flex_1().min_w(0).h(32).px(10).rounded(7).border_1().border_color(this.renameFocused ? t.accent : t.input).bg(t.background).text_size(13).text_color(t.foreground))
          .child(cancel).child(this.button(t, `st-m-save-${m.name}`, "Save", (_e, cx) => this.machineAction(m.name, "rename", cx, edit.name)))));
    }
    if (result) row.child(div().text_size(12.5).text_color(result.ok ? t.ansi2 : t.blocked).child(result.message));
    return row;
  }

  machines(t) {
    const version = updates().core.version;
    return v_flex().w_full()
      .child(this.heading(t, "Machines"))
      .child(div().pb(8).text_size(12.5).line_height("18px").text_color(t.dim).child("Your computers, reached over ssh. Their terminals keep running when you disconnect."))
      .child(this.row(t, "This computer", `Connected · ${version ? `hc ${version}` : "hc version unknown"}`, div().w(7).h(7).rounded(4).bg(t.ansi2)))
      .children((this.machineList ?? []).map((m) => this.machineRow(t, m, version)))
      .when(this.machineList === null && !this.machineError, (el) => el.child(h_flex().py(14).gap(8).items_center().child(svg("mods/workspace-chrome/spinner.svg").size(12).text_color(t.working).spin(900)).child(div().text_size(12.5).text_color(t.dim).child("Reading machines…"))))
      .when(this.machineList?.length === 0, (el) => el.child(div().py(14).text_size(12.5).text_color(t.dim).child("No other machines yet.")))
      .when(Boolean(this.machineError), (el) => el.child(div().py(10).text_size(12.5).text_color(t.blocked).child(this.machineError)))
      .when(Boolean(this.machineNotice), (el) => el.child(div().py(10).text_size(12.5).text_color(t.ansi2).child(this.machineNotice)))
      .child(h_flex().pt(16).child(this.button(t, "st-m-add", "Add a machine", () => run("mod.open", { name: "machine-add" }))));
  }

  // Alerts ---------------------------------------------------------------

  alerts(t) {
    const sounds = this.person.sounds ?? {};
    const on = sounds.enabled !== false;
    const file = (f) => (f ? f.split(/[\\/]/).pop() : "Herdr's sound");
    return v_flex()
      .w_full()
      .child(this.heading(t, "Sounds"))
      .child(
        this.row(
          t,
          "Play sounds",
          "When an agent needs you, and when one finishes while you're not looking. A note shows in the corner either way.",
          this.toggle(t, "st-sounds", "Play sounds", on, (_e, cx) => this.change((p) => (p.sounds = { ...(p.sounds ?? {}), enabled: !on }), cx)),
        ),
      )
      .child(this.row(t, "An agent needs you", file(sounds.blocked), this.button(t, "st-play-blocked", "Play", () => playSound("blocked"))))
      .child(this.row(t, "An agent finished", file(sounds.done), this.button(t, "st-play-done", "Play", () => playSound("done"))))
      .child(div().pt(12).text_size(12.5).line_height("18px").text_color(t.dim).child("Want your own sounds? Ask your agent to set sounds.done and sounds.blocked in settings.json to an MP3."));
  }

  // Updates --------------------------------------------------------------

  updates(t) {
    const u = updates();
    const at = (secs) => {
      const d = new Date(secs * 1000);
      return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
    };
    const checked = u.checked ? ` Checked at ${at(u.checked)}.` : "";
    const result = u.checking ? "Checking\u2026" : u.checked
      ? `Up to date.${checked}`
      : "No successful check. Check your connection and try again.";
    // Pressed: "Checking\u2026" while it looks, then "Up to date" for a moment
    // (a find changes the row instead). One per row: an element shows once.
    const asked = this.asked && Date.now() - this.asked < 3000;
    const check = (id) =>
      !u.checks
        ? null
        : u.checking && this.asked
          ? div().px(12).py(5).text_size(13).text_color(t.dim).child("Checking\u2026")
          : asked && u.checked && !u.checking
            ? div().px(12).py(5).text_size(13).text_color(t.ansi2).child("\u2713 Up to date")
            : this.button(t, id, "Check now", (_e, cx) => {
                this.asked = Date.now();
                run("app.check_updates");
                cx.notify();
              });
    const primary = (id, label, f) =>
      div().id(id).px(12).py(5).rounded(7).cursor_pointer().bg(t.accent).text_color(t.accent_foreground).text_size(13).font_semibold().hover((s) => s.opacity(0.9)).on_click(f).child(label);
    const app = !u.app.updates
      ? ["A build from source: it updates with git, not here.", null]
      : u.app.ready
        ? [`${u.app.ready} is ready. Restart to switch now; it also switches when you quit.`, primary("st-app-update", "Restart to update", () => run("app.update"))]
        : [result, check("st-check-app")];
    const c = u.core;
    const core = c.managed
      ? [`Managed by ${c.managed}: update it there.${c.ready ? ` ${c.ready} is available.` : ""}`, u.checks ? check("st-check-core") : null]
      : c.updating
        ? ["Updating\u2026 your terminals keep running.", null]
        : c.error
          ? [`The update failed: ${c.error}`, primary("st-core-retry", "Try again", () => run("app.update_core"))]
          : c.ready
            ? [`${c.ready} is ready. Your terminals keep running through it.`, primary("st-core-update", "Update herdr-core", () => run("app.update_core"))]
            : u.checks
              ? [result, check("st-check-core")]
              : ["Not checked: a build from source looks for no updates.", null];
    return v_flex()
      .w_full()
      .child(this.heading(t, "Herdr"))
      .child(this.row(t, `Herdr ${u.app.version}`, app[0], h_flex().gap(8).flex_none()
        .child(this.button(t, "st-whats-new", "What's new", () => { closeOverlay(props()?.overlay); run("app.whats_new"); }))
        .when(Boolean(app[1]), (el) => el.child(app[1]))))
      .child(div().h(18))
      .child(this.heading(t, "herdr-core"))
      .child(this.row(t, `herdr-core ${c.version ?? ""}`, core[0], core[1] ?? div()))
      .child(div().pt(12).text_size(12.5).line_height("18px").text_color(t.dim).child("Herdr looks for both every hour and tells you in the corner. herdr-core runs your terminals, so they keep running through its updates."));
  }


  // Privacy --------------------------------------------------------------

  // A report as it goes out, one line per line of JSON.
  report(t, id, title, about, report) {
    const lines = JSON.stringify(report, null, 2).split("\n");
    return v_flex()
      .w_full()
      .pt(14)
      .gap(4)
      .child(div().text_size(13.5).text_color(t.foreground).child(title))
      .when(Boolean(about), (el) => el.child(div().text_size(12.5).text_color(t.dim).child(about)))
      .child(
        v_flex()
          .id(id)
          .mt(4)
          .w_full()
          .p(10)
          .rounded(8)
          .border_1()
          .border_color(t.border)
          .bg(t.background)
          .children(lines.map((l) => div().font_family(DEFAULT_FONT).text_size(12).line_height("17px").text_color(t.muted).whitespace_nowrap().child(l || " "))),
      );
  }

  privacy(t) {
    const s = telemetry();
    const on = s ? s.setting : this.person.telemetry !== false;
    const at = (secs) => new Date(secs * 1000).toLocaleString();
    const why = s?.off_because ? `Off for now: ${s.off_because} is set where Herdr started.` : null;
    const body = v_flex()
      .w_full()
      .child(this.heading(t, "Usage data"))
      .child(
        this.row(
          t,
          "Share anonymous usage data",
          why ?? "Once a day: which agents and features you use, how many spaces and tabs, your system and Herdr's version. Never what's in your terminals, never names of files, folders or projects.",
          this.toggle(t, "st-telemetry", "Share anonymous usage data", on, (_e, cx) =>
            this.change((p) => {
              if (on) p.telemetry = false;
              else delete p.telemetry;
            }, cx),
          ),
        ),
      )
      .child(
        this.row(
          t,
          "What we send",
          s && !s.sends ? "This build never sends: it's built from source. You can still see what a release would send." : "Exactly what leaves your computer, word for word.",
          this.button(t, "st-telemetry-show", this.showData ? "Hide" : "Show", (_e, cx) => {
            this.showData = !this.showData;
            cx.notify();
          }),
        ),
      );
    if (!this.showData) return body;
    if (!s?.enabled) return body.child(div().pt(14).text_size(13).text_color(t.dim).child("Nothing is collected or sent while it's off."));
    return body
      .children(s.queued.map((r, i) => this.report(t, `st-tq-${i}`, `Waiting to send: ${r.day}`, "Sent as is, soon.", r)))
      .when(Boolean(s.draft), (el) => el.child(this.report(t, "st-td", "Today so far", "A draft: it changes until the day ends (UTC), then goes out once.", s.draft)))
      .when(Boolean(s.last_sent), (el) => el.child(this.report(t, "st-tl", `Last sent: ${s.last_sent.report.day}`, `Sent ${at(s.last_sent.at)}.`, s.last_sent.report)));
  }

  // ----------------------------------------------------------------------

  render(cx) {
    const t = theme();
    const u = updates();
    const waiting = Boolean(u.app.ready || (u.core.ready && !u.core.managed));
    const nav = v_flex()
      .flex_none()
      .w(190)
      .p(10)
      .gap(2)
      .border_r(1)
      .border_color(t.border)
      .child(div().px(10).pt(6).pb(12).text_size(16).font_semibold().text_color(t.foreground).child("Settings"))
      .children(
        SECTIONS.map((s) => {
          const on = s.id === this.section;
          return h_flex()
            .id(`st-s-${s.id}`)
            .gap(10)
            .px(10)
            .py(7)
            .items_center()
            .rounded(7)
            .cursor_pointer()
            .bg(on ? t.raised : "#00000000")
            .hover((st) => st.bg(t.raised))
            .on_click((_e, cx) => this.leaveShortcut(cx, (cx) => {
              this.section = s.id;
              this.picking = null;
              this.machineEdit = null;
              this.handle.focus();
              if (s.id === "machines") cx.spawn(async (cx) => { await this.loadMachines(); cx.notify(); });
              cx.notify();
            }))
            .child(svg(s.icon).size(15).text_color(on ? t.accent : t.dim))
            .child(div().flex_1().text_size(13.5).text_color(on ? t.foreground : t.muted).child(s.title))
            .when(s.id === "updates" && waiting, (el) => el.child(div().w(7).h(7).rounded(4).bg(t.accent)));
        }),
      );
    return h_flex()
      .id("settings")
      .track_focus(this.handle)
      .when(Boolean(this.picking), (el) => el.on_action("overlay.close", (_e, cx) => {
        this.picking = null;
        this.handle.focus();
        cx.stop_propagation();
        cx.notify();
      }))
      .when(Boolean(this.shortcutEdit), (el) => el.on_action("overlay.close", (_e, cx) => {
        this.cancelShortcut(cx, true);
        cx.stop_propagation();
      }))
      .w_full()
      .h(620)
      .min_h(0)
      .items_start()
      .on_key_down((e, cx) => {
        const k = e.key;
        if (this.shortcutEdit && k === "escape") {
          this.cancelShortcut(cx, true);
          cx.stop_propagation();
          return;
        }
        if (this.machineEdit) {
          if (k === "escape") this.machineEdit = null;
          else return;
          this.handle.focus();
          cx.stop_propagation();
          cx.notify();
          return;
        }
        if (k === "escape") {
          if (this.picking) this.picking = null;
          else return this.close(cx);
          this.handle.focus();
        } else return;
        cx.stop_propagation();
        cx.notify();
      })
      .child(nav.h_full().child(div().flex_1()).child(this.button(t, "st-feedback", "Send feedback", (_e, cx) => this.leaveShortcut(cx, () => { closeOverlay(props()?.overlay); run("app.feedback"); }))))
      .child(
        // A bar that always shows: the pages are taller than the dialog and
        // people should see that they scroll.
        div()
          .relative()
          .flex_1()
          .min_w(0)
          .h_full()
          .child(
            div()
              .id("st-body")
              .size_full()
              .overflow_y_scroll()
              // Padding inside, not on the scroll area: there it counts as
              // content and a page that fits still shows a bar.
              .child(
                div()
                  .px(28)
                  .py(22)
                  .child(
                    this.section === "look"
                      ? this.look(t, cx)
                      : this.section === "quick-terminal"
                        ? this.quickTerminal(t)
                      : this.section === "keyboard"
                        ? this.keyboard(t)
                      : this.section === "machines"
                        ? this.machines(t)
                      : this.section === "alerts"
                        ? this.alerts(t)
                        : this.section === "privacy"
                          ? this.privacy(t)
                          : this.updates(t),
                  ),
              ),
          )
          .child(Scrollbar.vertical("st-body").mode("always").absolute().inset_0()),
      );
  }
}
