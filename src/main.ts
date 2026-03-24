import { invoke } from "@tauri-apps/api/core";

interface ScrollSettings {
  enabled: boolean;
  scroll_speed: number;
  acceleration: number;
  animation_duration: number;
  inertia: boolean;
  inertia_decay: number;
  easing: "Linear" | "EaseOut" | "EaseInOut";
}

interface EngineStatus {
  running: boolean;
  accessibility_granted: boolean;
  enabled: boolean;
}

const SLIDERS: [string, string, string, number, number, (v: number) => string][] = [
  ["scroll-speed", "speed-fill", "speed-value", 0.5, 10, (v) => `${v.toFixed(1)}x`],
  ["acceleration", "accel-fill", "accel-value", 0, 1, (v) => v.toFixed(1)],
  ["duration", "duration-fill", "duration-value", 50, 800, (v) => `${v}ms`],
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
  try {
    await invoke("start_scroll_engine");
    retries = 0;
    return true;
  } catch {
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
    await applyStatus(s);
  } catch { /* ignore */ }
  finally { polling = false; }
}

// ── Settings ──────────────────────────

async function load() {
  try {
    const s: ScrollSettings = await invoke("get_settings");
    $in("enabled-toggle").checked = s.enabled;
    $in("scroll-speed").value = String(s.scroll_speed);
    $in("acceleration").value = String(s.acceleration);
    $in("duration").value = String(s.animation_duration);
    $in("inertia-toggle").checked = s.inertia;
    $in("inertia-decay").value = String(s.inertia_decay);
    const r = document.querySelector(`input[name="easing"][value="${s.easing}"]`) as HTMLInputElement | null;
    if (r) r.checked = true;
    syncFills();
    setDecay(s.inertia);
  } catch { /* first load may fail */ }
}

function setDecay(on: boolean) {
  const el = $("decay-setting");
  el.classList.toggle("disabled", !on);
  if (!on) $in("inertia-decay").setAttribute("tabindex", "-1");
  else $in("inertia-decay").removeAttribute("tabindex");
}

let saveTimer: ReturnType<typeof setTimeout>;

function save() {
  syncFills();
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    const easing = document.querySelector('input[name="easing"]:checked') as HTMLInputElement | null;
    try {
      await invoke("update_settings", {
        settings: {
          enabled: $in("enabled-toggle").checked,
          scroll_speed: parseFloat($in("scroll-speed").value),
          acceleration: parseFloat($in("acceleration").value),
          animation_duration: parseFloat($in("duration").value),
          inertia: $in("inertia-toggle").checked,
          inertia_decay: parseFloat($in("inertia-decay").value),
          easing: easing?.value ?? "EaseOut",
        },
      });
    } catch { /* ignore */ }
  }, 150);
}

// ── Init ──────────────────────────────

window.addEventListener("DOMContentLoaded", () => {
  load();
  poll();
  setInterval(poll, 2000);

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

  $in("inertia-toggle").addEventListener("change", () => {
    setDecay($in("inertia-toggle").checked);
    save();
  });

  for (const [sid] of SLIDERS) {
    $in(sid).addEventListener("input", save);
  }

  document.querySelectorAll('input[name="easing"]').forEach((r) => {
    r.addEventListener("change", save);
  });

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
