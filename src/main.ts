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

// Slider config: [id, fillId, badgeId, min, max, formatter]
const SLIDERS: [string, string, string, number, number, (v: number) => string][] = [
  ["scroll-speed", "speed-fill", "speed-value", 0.5, 10, (v) => `${v.toFixed(1)}x`],
  ["acceleration", "accel-fill", "accel-value", 0, 1, (v) => v.toFixed(1)],
  ["duration", "duration-fill", "duration-value", 50, 800, (v) => `${v}ms`],
  ["inertia-decay", "decay-fill", "decay-value", 0.8, 0.99, (v) => v.toFixed(2)],
];

const MAX_START_RETRIES = 3;
let startRetries = 0;
let isPolling = false;
// @ts-ignore: stored for cleanup on window unload
let _pollTimer: ReturnType<typeof setInterval> | null = null;
let fastPollTimer: ReturnType<typeof setInterval> | null = null;

function $(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (!el) throw new Error(`Element #${id} not found`);
  return el;
}

function $input(id: string): HTMLInputElement {
  return $(id) as HTMLInputElement;
}

// ─── Slider fill ────────────────────────────────────────────────────

function updateSliderFill(sliderId: string, fillId: string, min: number, max: number) {
  const val = parseFloat($input(sliderId).value);
  const pct = ((val - min) / (max - min)) * 100;
  $(fillId).style.width = `${pct}%`;
}

function updateAllSliderFills() {
  for (const [sliderId, fillId, , min, max] of SLIDERS) {
    updateSliderFill(sliderId, fillId, min, max);
  }
}

// ─── Engine start ───────────────────────────────────────────────────

async function tryStartEngine(): Promise<boolean> {
  try {
    await invoke("start_scroll_engine");
    startRetries = 0;
    return true;
  } catch (e) {
    console.warn("Engine start failed:", e);
    return false;
  }
}

// ─── Status display ─────────────────────────────────────────────────

async function updateStatusUI(status: EngineStatus) {
  const dot = $("status-dot").querySelector(".dot");
  if (!dot) return;
  const sublabel = $("status-text");
  const banner = $("permission-banner");

  if (!status.accessibility_granted) {
    dot.className = "dot warning";
    sublabel.textContent = "Needs permission";
    sublabel.className = "status-sublabel warning";
    banner.style.display = "flex";
    startRetries = 0;
  } else if (status.enabled && status.running) {
    dot.className = "dot active";
    sublabel.textContent = "Running";
    sublabel.className = "status-sublabel";
    banner.style.display = "none";
    startRetries = 0;
  } else if (status.enabled && !status.running) {
    banner.style.display = "none";
    if (startRetries < MAX_START_RETRIES) {
      dot.className = "dot warning";
      sublabel.textContent = "Starting...";
      sublabel.className = "status-sublabel warning";
      startRetries++;
      const started = await tryStartEngine();
      if (started) {
        dot.className = "dot active";
        sublabel.textContent = "Running";
        sublabel.className = "status-sublabel";
      }
    } else {
      dot.className = "dot inactive";
      sublabel.textContent = "Failed to start";
      sublabel.className = "status-sublabel warning";
    }
  } else {
    dot.className = "dot inactive";
    sublabel.textContent = "Paused";
    sublabel.className = "status-sublabel inactive";
    banner.style.display = "none";
    startRetries = 0;
  }
}

// ─── Settings I/O ───────────────────────────────────────────────────

async function loadSettings() {
  try {
    const settings: ScrollSettings = await invoke("get_settings");

    $input("enabled-toggle").checked = settings.enabled;
    $input("scroll-speed").value = String(settings.scroll_speed);
    $input("acceleration").value = String(settings.acceleration);
    $input("duration").value = String(settings.animation_duration);
    $input("inertia-toggle").checked = settings.inertia;
    $input("inertia-decay").value = String(settings.inertia_decay);

    const easing = document.querySelector(
      `input[name="easing"][value="${settings.easing}"]`
    ) as HTMLInputElement | null;
    if (easing) easing.checked = true;

    updateBadges();
    updateAllSliderFills();
    updateDecayVisibility(settings.inertia);
  } catch (e) {
    console.error("Failed to load settings:", e);
  }
}

function updateBadges() {
  for (const [sliderId, , badgeId, , , fmt] of SLIDERS) {
    const val = parseFloat($input(sliderId).value);
    $(badgeId).textContent = fmt(val);
  }
}

function getCurrentSettings(): ScrollSettings {
  const easing = document.querySelector(
    'input[name="easing"]:checked'
  ) as HTMLInputElement | null;

  return {
    enabled: $input("enabled-toggle").checked,
    scroll_speed: parseFloat($input("scroll-speed").value),
    acceleration: parseFloat($input("acceleration").value),
    animation_duration: parseFloat($input("duration").value),
    inertia: $input("inertia-toggle").checked,
    inertia_decay: parseFloat($input("inertia-decay").value),
    easing: (easing?.value as ScrollSettings["easing"]) || "EaseOut",
  };
}

function updateDecayVisibility(enabled: boolean) {
  const group = $("decay-setting");
  const slider = $input("inertia-decay");
  if (enabled) {
    group.classList.remove("disabled");
    slider.removeAttribute("tabindex");
  } else {
    group.classList.add("disabled");
    slider.setAttribute("tabindex", "-1");
  }
}

let saveTimeout: ReturnType<typeof setTimeout>;

function saveSettings() {
  updateBadges();
  updateAllSliderFills();

  // Debounce at 150ms to avoid hammering the backend during slider drags
  clearTimeout(saveTimeout);
  saveTimeout = setTimeout(async () => {
    try {
      const settings = getCurrentSettings();
      await invoke("update_settings", { settings });
    } catch (e) {
      console.error("Failed to save settings:", e);
    }
  }, 150);
}

// ─── Status polling (with re-entrance guard) ────────────────────────

async function pollStatus() {
  if (isPolling) return;
  isPolling = true;
  try {
    const status: EngineStatus = await invoke("get_engine_status");
    await updateStatusUI(status);
  } catch (e) {
    console.error("Status poll failed:", e);
  } finally {
    isPolling = false;
  }
}

// ─── Init ───────────────────────────────────────────────────────────

window.addEventListener("DOMContentLoaded", () => {
  loadSettings();
  pollStatus();

  // Single poll interval — never stacked
  _pollTimer = setInterval(pollStatus, 2000);

  // Master toggle
  $input("enabled-toggle").addEventListener("change", async () => {
    const enabled = $input("enabled-toggle").checked;
    saveSettings();

    if (enabled) {
      startRetries = 0;
      await tryStartEngine();
    } else {
      try {
        await invoke("stop_scroll_engine");
      } catch (e) {
        console.error("Failed to stop engine:", e);
      }
    }
    pollStatus();
  });

  // Inertia toggle
  $input("inertia-toggle").addEventListener("change", () => {
    updateDecayVisibility($input("inertia-toggle").checked);
    saveSettings();
  });

  // Range sliders
  for (const [sliderId] of SLIDERS) {
    $input(sliderId).addEventListener("input", () => saveSettings());
  }

  // Easing radios
  document.querySelectorAll('input[name="easing"]').forEach((radio) => {
    radio.addEventListener("change", () => saveSettings());
  });

  // Permission button — opens System Settings, fast-polls for 30s
  $("open-accessibility-btn").addEventListener("click", async () => {
    try {
      await invoke("open_accessibility_settings");
      // Clear any existing fast poll before starting a new one
      if (fastPollTimer) clearInterval(fastPollTimer);
      let fastPolls = 0;
      fastPollTimer = setInterval(async () => {
        await pollStatus();
        fastPolls++;
        if (fastPolls >= 15) {
          if (fastPollTimer) clearInterval(fastPollTimer);
          fastPollTimer = null;
        }
      }, 2000);
    } catch (e) {
      console.error("Failed to open accessibility settings:", e);
    }
  });
});
