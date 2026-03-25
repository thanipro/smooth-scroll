# Smooth Scroll

A macOS utility that adds smooth, fluid scrolling to any mouse — built with **Tauri v2** and **Rust**.

## The Story

I was about to renew my SmoothScroll app subscription — $12/year just to make my mouse scroll smoothly. Then I thought: why am I paying for this? So I sat down and built this in about an hour.

I also tried other free smooth scrolling apps, but none of them worked properly with Logi Options+ — they all skip continuous scroll events from the Logitech daemon, so scrolling just passes through unsmoothed. This one detects Logi Options+ events via scroll/momentum phase and handles them correctly.

At least I don't have to pay for a scroll utility subscription anymore.

## What It Does

Intercepts choppy, discrete scroll events from a regular mouse and replaces them with smooth, velocity-based pixel scrolling — the same feel you get from a trackpad or Magic Mouse.

- **Works with Logitech mice** — detects Logi Options+ continuous events via scroll/momentum phase detection
- **Trackpad scrolling is left untouched** — real trackpad events (with non-zero phase) pass through unmodified
- **Works system-wide** across all apps
- **Menu bar only** — no dock icon, runs as a lightweight tray utility (~5MB)
- **Close = hide** — closing the settings window hides it; quit from the tray menu
- **Launch at Login** — toggle from the tray menu

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                    Tauri v2 App                      │
│                                                      │
│  ┌──────────────┐     Tauri Commands     ┌────────┐ │
│  │  Webview UI   │ ◄──────────────────► │  Rust   │ │
│  │  (Settings)   │    get/update         │ Backend │ │
│  │  HTML/CSS/TS  │    settings           │         │ │
│  └──────────────┘                        └────┬───┘ │
│                                               │      │
│  ┌────────────────────────────────────────────┘      │
│  │                                                    │
│  │  ┌─────────────────┐    ┌──────────────────────┐  │
│  │  │  CGEventTap      │    │  CVDisplayLink       │  │
│  │  │  (HID level)     │    │  (display-synced)    │  │
│  │  │                  │    │                      │  │
│  │  │  Intercepts raw  │───►│  Emits velocity-     │  │
│  │  │  scroll events   │    │  based smooth pixel  │  │
│  │  │  from mouse      │    │  scroll events at    │  │
│  │  │                  │    │  monitor refresh     │  │
│  │  │                  │    │  rate (60/120Hz)     │  │
│  │  └─────────────────┘    └──────────────────────┘  │
│  │         │                         │                │
│  │    Suppresses                Posts to               │
│  │    original                  kCGSessionEventTap     │
│  │    event                     (tagged with           │
│  │                              sentinel marker)       │
│  └────────────────────────────────────────────────────┘
└─────────────────────────────────────────────────────┘
```

### Key Design Decisions

**Velocity-Based Scroll Model** — Each mouse wheel tick adds an impulse to a velocity accumulator. Every display frame emits `velocity` pixels of scroll, then decays velocity. This produces smooth, consistent movement like a native macOS trackpad — no choppy position-animation stepping.

**Logi Options+ Compatibility** — Logi Options+ converts discrete wheel ticks into continuous pixel events (`isContinuous=1`). We distinguish these from real trackpad events by checking `scrollPhase` and `momentumPhase` — Logi events have both at 0, while real trackpad events have non-zero phases. Pixel deltas are read from `PointDelta` fields (96/97) instead of the integer `Delta` fields (11/12).

**Display-Synced Animation** — Uses `CVDisplayLink` instead of `thread::sleep`. The callback fires on a high-priority CoreVideo thread perfectly synced to the monitor's refresh rate (60Hz, 120Hz ProMotion, or variable). Falls back to a 120fps sleep loop if CVDisplayLink fails.

**Frame-Rate Independence** — Decay is calculated using `mach_absolute_time` to measure real elapsed time between frames, then adjusting the decay exponent accordingly. Scrolling feels identical on 60Hz, 120Hz, and variable refresh displays.

**Infinite Loop Prevention** — Two layers:
1. Synthetic events are posted to `kCGSessionEventTap` (downstream of our HID-level tap, so they never reach our callback)
2. Events are tagged with a sentinel value via `eventSourceUserData` (field 42) as a belt-and-suspenders guard

**Priority Inversion Avoidance** — Uses `std::sync::Mutex` (not `parking_lot`) because macOS's `pthread_mutex` supports priority inheritance. The event tap callback runs on a high-priority thread, so `parking_lot`'s userspace spinlock would cause priority inversion.

**Scroll Physics**:
- *Impulse ramp* — new impulses are fed into velocity over 4 frames for smooth starts
- *Two-phase decay* — faster decay at high velocity, slower glide tail for natural feel
- *Direction reversal* — opposite momentum is killed instantly for responsive direction changes
- *Per-frame pixel cap* — time-based (4800px/s) to prevent jarring jumps, consistent across refresh rates
- *Sub-pixel precision* — fractional remainders carried between frames to prevent drift

## Settings

| Setting | Range | Description |
|---------|-------|-------------|
| Scroll Speed | 0.5x – 10x | Impulse multiplier per wheel tick |
| Acceleration | 0 – 1 | Extra speed boost for fast scroll gestures |
| Glide | 0.80 – 0.99 | Velocity decay per frame — higher = more momentum |

## Installation

### From GitHub Releases

1. Download the `.dmg` from the [latest release](https://github.com/thanipro/smooth-scroll/releases)
2. Open the DMG and drag **Smooth Scroll** to Applications
3. The app is not notarized yet, so macOS will block it. Run this once in Terminal:
   ```bash
   xattr -cr "/Applications/Smooth Scroll.app"
   ```
4. Launch Smooth Scroll — it will prompt for Accessibility permission on first run

### Build from source

```bash
git clone https://github.com/thanipro/smooth-scroll.git
cd smooth-scroll
npm install
npm run tauri build
```

The `.app` and `.dmg` will be in `src-tauri/target/release/bundle/`.

## Requirements

- macOS 10.15+
- **Accessibility permission** — required for `CGEventTap` to intercept scroll events. The app prompts you to grant this on first launch.

## Development

```bash
# Install dependencies
npm install

# Run in development mode
npm run tauri dev

# Build release .app and .dmg
npm run tauri build
```

### Project Structure

```
src-tauri/src/
├── lib.rs            # Tauri commands, tray menu, app lifecycle
├── logging.rs        # Shared debug logging (dbg_log! macro)
├── main.rs           # Entry point
└── scroll/
    ├── mod.rs        # Public API, accessibility helpers
    ├── ffi.rs        # CoreGraphics/CoreVideo/mach FFI bindings
    ├── state.rs      # ScrollSettings, ScrollState, physics constants
    ├── physics.rs    # Frame processing, event callbacks
    └── engine.rs     # ScrollEngine lifecycle, thread management
```

### Stack

- **Frontend**: Vanilla TypeScript + CSS (no framework)
- **Backend**: Rust with raw CoreGraphics/CoreVideo FFI
- **Framework**: Tauri v2
- **Build**: Vite + Cargo
- **CI**: GitHub Actions — builds DMG on release

## License

MIT
