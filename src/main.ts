import { invoke } from "@tauri-apps/api/core";

function log(...args: unknown[]) {
  console.log("[SmoothScroll UI]", ...args);
}

interface ScrollSettings {
  enabled: boolean;
  scroll_speed: number;
  acceleration: number;
  inertia_decay: number;
}

interface EngineStatus {
  running: boolean;
  accessibility_granted: boolean;
  enabled: boolean;
}

const SLIDERS: [string, string, string, number, number, (v: number) => string][] = [
  ["scroll-speed", "speed-fill", "speed-value", 0.5, 10, (v) => `${v.toFixed(1)}x`],
  ["acceleration", "accel-fill", "accel-value", 0, 1, (v) => v.toFixed(1)],
  ["inertia-decay", "decay-fill", "decay-value", 0.8, 0.99, (v) => v.toFixed(2)],
];

const MAX_RETRIES = 3;
let retries = 0;
let polling = false;
let fastPoll: ReturnType<typeof setInterval> | null = null;

function $(id: string) {
  const el = document.getElementById(id);
  if (!el) throw new Error(`#${id} not found`);
  return el;
}

function $in(id: string) {
  return $(id) as HTMLInputElement;
}

// ── Slider fills ──────────────────────

function syncFills() {
  for (const [sid, fid, bid, min, max, fmt] of SLIDERS) {
    const v = parseFloat($in(sid).value);
    $(fid).style.width = `${((v - min) / (max - min)) * 100}%`;
    $(bid).textContent = fmt(v);
  }
}

// ── Engine ────────────────────────────

async function tryStart(): Promise<boolean> {
  log("tryStart: invoking start_scroll_engine...");
  try {
    await invoke("start_scroll_engine");
    retries = 0;
    log("tryStart: success");
    return true;
  } catch (e) {
    log("tryStart: FAILED", e);
    return false;
  }
}

// ── Status ────────────────────────────

async function applyStatus(s: EngineStatus) {
  const dot = $("status-dot");
  const sub = $("status-text");
  const banner = $("permission-banner");

  if (!s.accessibility_granted) {
    dot.className = "indicator warn";
    sub.textContent = "Needs permission";
    sub.className = "row-sub warning";
    banner.style.display = "flex";
    retries = 0;
  } else if (s.enabled && s.running) {
    dot.className = "indicator";
    sub.textContent = "Running";
    sub.className = "row-sub";
    banner.style.display = "none";
    retries = 0;
  } else if (s.enabled && !s.running) {
    banner.style.display = "none";
    if (retries < MAX_RETRIES) {
      dot.className = "indicator warn";
      sub.textContent = "Starting...";
      sub.className = "row-sub warning";
      retries++;
      if (await tryStart()) {
        dot.className = "indicator";
        sub.textContent = "Running";
        sub.className = "row-sub";
      }
    } else {
      dot.className = "indicator off";
      sub.textContent = "Could not start";
      sub.className = "row-sub warning";
    }
  } else {
    dot.className = "indicator off";
    sub.textContent = "Off";
    sub.className = "row-sub inactive";
    banner.style.display = "none";
    retries = 0;
  }
}

async function poll() {
  if (polling) return;
  polling = true;
  try {
    const s: EngineStatus = await invoke("get_engine_status");
    log("poll: status →", s);
    await applyStatus(s);
  } catch (e) {
    log("poll: error fetching status", e);
  }
  finally { polling = false; }
}

// ── Settings ──────────────────────────

async function load() {
  log("load: fetching settings...");
  try {
    const s: ScrollSettings = await invoke("get_settings");
    log("load: settings →", s);
    $in("enabled-toggle").checked = s.enabled;
    $in("scroll-speed").value = String(s.scroll_speed);
    $in("acceleration").value = String(s.acceleration);
    $in("inertia-decay").value = String(s.inertia_decay);
    syncFills();
  } catch (e) {
    log("load: FAILED (may be normal on first load)", e);
  }
}

let saveTimer: ReturnType<typeof setTimeout>;

function save() {
  syncFills();
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    const payload = {
      enabled: $in("enabled-toggle").checked,
      scroll_speed: parseFloat($in("scroll-speed").value),
      acceleration: parseFloat($in("acceleration").value),
      inertia_decay: parseFloat($in("inertia-decay").value),
    };
    log("save: updating settings →", payload);
    try {
      await invoke("update_settings", { settings: payload });
      log("save: success");
    } catch (e) {
      log("save: FAILED", e);
    }
  }, 150);
}

// ── Shortcuts ─────────────────────────

type WindowOp = "left_half" | "right_half" | "maximize" | "center" | "next_display";
type ShortcutAction =
  | { type: "app"; name: string; toggle: boolean }
  | { type: "window"; op: WindowOp };

const WINDOW_LABELS: Record<WindowOp, string> = {
  left_half: "Window: left half",
  right_half: "Window: right half",
  maximize: "Window: maximize",
  center: "Window: centre",
  next_display: "Window: next display",
};

function actionLabel(a: ShortcutAction): string {
  return a.type === "app" ? a.name : WINDOW_LABELS[a.op];
}
interface ShortcutBinding { id: string; keys: string; action: ShortcutAction }
interface ShortcutAvailability { available: boolean; status: string; detail: string }
interface InstalledApp { name: string; path: string }

const MOD_SYMBOLS: Record<string, string> = { Control: "⌃", Alt: "⌥", Shift: "⇧", Command: "⌘" };

/** "Alt+Shift+S" → "⌥⇧S" for display. */
function prettyKeys(keys: string): string {
  return keys.split("+").map((t) => MOD_SYMBOLS[t] ?? t).join("");
}

/** Turn a KeyboardEvent into global-hotkey syntax, or null if it is only modifiers. */
function comboFromEvent(e: KeyboardEvent): string | null {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Control");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Command");
  const c = e.code;
  let key: string | null = null;
  if (/^Key[A-Z]$/.test(c)) key = c.slice(3);
  else if (/^Digit[0-9]$/.test(c)) key = c.slice(5);
  else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(c)) key = c;
  else if (["Space", "Enter", "Tab", "Escape", "Backspace", "Delete", "Home", "End", "PageUp", "PageDown",
            "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Minus", "Equal", "Comma", "Period", "Slash",
            "Semicolon", "Quote", "BracketLeft", "BracketRight", "Backslash", "Backquote"].includes(c)) key = c;
  if (!key) return null;
  if (mods.length === 0) return null; // refuse bare keys: they would swallow normal typing
  return [...mods, key].join("+");
}

let recordedKeys: string | null = null;
let recording = false;

function setHint(text: string, cls = "") {
  const h = $("shortcut-hint");
  h.textContent = text;
  h.className = "hint" + (cls ? ` ${cls}` : "");
}

async function renderShortcuts() {
  const list = $("shortcut-list");
  let items: ShortcutBinding[] = [];
  try { items = await invoke("list_shortcuts"); } catch (e) { log("list_shortcuts failed", e); }
  list.innerHTML = "";
  for (const s of items) {
    const li = document.createElement("li");
    li.className = "shortcut-row";
    const kbd = document.createElement("kbd");
    kbd.textContent = prettyKeys(s.keys);
    const name = document.createElement("span");
    name.className = "app-name";
    name.textContent = actionLabel(s.action);
    let toggleWrap: HTMLElement | null = null;
    if (s.action.type === "app") {
      const action = s.action;
      toggleWrap = document.createElement("label");
      toggleWrap.className = "toggle-label";
      toggleWrap.innerHTML = `<span>Toggle</span><label class="switch switch-sm"><input type="checkbox" ${action.toggle ? "checked" : ""} aria-label="Hide when already in front"><span class="switch-track"></span></label>`;
      const cb = toggleWrap.querySelector("input") as HTMLInputElement;
      cb.addEventListener("change", async () => {
        try {
          await invoke("update_shortcut", { id: s.id, action: { ...action, toggle: cb.checked } });
        } catch (e) { log("update_shortcut failed", e); }
      });
    }
    const rm = document.createElement("button");
    rm.className = "btn-remove";
    rm.type = "button";
    rm.textContent = "×";
    rm.setAttribute("aria-label", `Remove ${s.keys}`);
    rm.addEventListener("click", async () => {
      try { await invoke("remove_shortcut", { id: s.id }); } catch (e) { log("remove_shortcut failed", e); }
      renderShortcuts();
    });
    li.append(kbd, name);
    if (toggleWrap) li.append(toggleWrap);
    li.append(rm);
    list.appendChild(li);
  }
}

async function populateApps() {
  const picker = $("app-picker") as HTMLSelectElement;
  if (picker.options.length > 0) return;
  let apps: InstalledApp[] = [];
  try { apps = await invoke("list_apps"); } catch (e) { log("list_apps failed", e); }
  for (const a of apps) {
    const o = document.createElement("option");
    o.value = a.name;
    o.textContent = a.name;
    picker.appendChild(o);
  }
}

function selectedAction(): ShortcutAction | null {
  const kind = ($("action-picker") as HTMLSelectElement).value;
  if (kind === "app") {
    const app = ($("app-picker") as HTMLSelectElement).value;
    return app ? { type: "app", name: app, toggle: true } : null;
  }
  return { type: "window", op: kind as WindowOp };
}

function updateAddState() {
  const kind = ($("action-picker") as HTMLSelectElement).value;
  ($("app-picker") as HTMLElement).style.display = kind === "app" ? "" : "none";
  ($("add-shortcut") as HTMLButtonElement).disabled = !(recordedKeys && selectedAction());
}

async function probeKeys(keys: string) {
  const rec = $("key-recorder");
  try {
    const r: ShortcutAvailability = await invoke("check_shortcut", { keys });
    rec.className = "key-recorder " + (r.available ? "ok" : "bad");
    setHint(r.detail, r.available ? "ok" : r.status === "used_here" ? "warning" : "error");
    if (!r.available) recordedKeys = null;
  } catch (e) {
    rec.className = "key-recorder bad";
    setHint(String(e), "error");
    recordedKeys = null;
  }
  updateAddState();
}

function initShortcutsUI() {
  const rec = $("key-recorder");
  const picker = $("app-picker") as HTMLSelectElement;

  rec.addEventListener("click", () => {
    recording = true;
    recordedKeys = null;
    rec.textContent = "Press keys…";
    rec.className = "key-recorder recording";
    setHint("Press a key combination with at least one modifier (⌥ ⌃ ⇧ ⌘).");
    updateAddState();
  });

  window.addEventListener("keydown", (e) => {
    if (!recording) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") {
      recording = false;
      rec.textContent = "Press keys…";
      rec.className = "key-recorder";
      setHint("");
      return;
    }
    const combo = comboFromEvent(e);
    if (!combo) {
      rec.textContent = [e.ctrlKey && "⌃", e.altKey && "⌥", e.shiftKey && "⇧", e.metaKey && "⌘"].filter(Boolean).join("") || "Press keys…";
      return;
    }
    recording = false;
    recordedKeys = combo;
    rec.textContent = prettyKeys(combo);
    probeKeys(combo);
  }, true);

  picker.addEventListener("change", updateAddState);
  $("action-picker").addEventListener("change", updateAddState);

  $("add-shortcut").addEventListener("click", async () => {
    const action = selectedAction();
    if (!recordedKeys || !action) return;
    try {
      await invoke("add_shortcut", { keys: recordedKeys, action });
      recordedKeys = null;
      rec.textContent = "Press keys…";
      rec.className = "key-recorder";
      setHint("Added.", "ok");
      updateAddState();
      renderShortcuts();
    } catch (e) {
      setHint(String(e), "error");
    }
  });

  renderShortcuts();
  populateApps().then(updateAddState);
}

// ── Init ──────────────────────────────

window.addEventListener("DOMContentLoaded", () => {
  log("DOM loaded — initializing");
  load();
  poll();
  setInterval(poll, 2000);

  // The window is hidden, not destroyed, when closed. Re-read settings each
  // time it comes back so tray toggles made while it was hidden are reflected.
  window.addEventListener("focus", () => {
    log("window focused — reloading settings");
    load();
    renderShortcuts();
  });

  initShortcutsUI();

  $in("enabled-toggle").addEventListener("change", async () => {
    save();
    if ($in("enabled-toggle").checked) {
      retries = 0;
      await tryStart();
    } else {
      try { await invoke("stop_scroll_engine"); } catch { /* */ }
    }
    poll();
  });

  for (const [sid] of SLIDERS) {
    $in(sid).addEventListener("input", save);
  }

  $("open-accessibility-btn").addEventListener("click", async () => {
    try { await invoke("open_accessibility_settings"); } catch { /* */ }
    if (fastPoll) clearInterval(fastPoll);
    let n = 0;
    fastPoll = setInterval(async () => {
      await poll();
      if (++n >= 15) { clearInterval(fastPoll!); fastPoll = null; }
    }, 2000);
  });
});
