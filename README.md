# Smooth Scroll

Smooth mouse scrolling, global app shortcuts and window snapping for macOS — one ~5 MB menu bar app, built with **Tauri v2** and **Rust**.

## The Story

I was about to renew my SmoothScroll app subscription — $12/year just to make my mouse scroll smoothly. Then I thought: why am I paying for this? So I sat down and built this in about an hour.

I also tried other free smooth scrolling apps, but none of them worked properly with Logi Options+ — they all skip continuous scroll events from the Logitech daemon, so scrolling just passes through unsmoothed. This one detects Logi Options+ events via scroll/momentum phase and handles them correctly.

Since the app already holds the Accessibility permission it needs for scroll interception, it turned out to be the natural home for the other small utilities I was paying for or installing separately: a hotkey to jump to Slack or the terminal, and window snapping. So those moved in too.

## What It Does

### 1. Smooth scrolling

Intercepts choppy, discrete scroll events from a regular mouse and replaces them with smooth, velocity-based scrolling — the same feel you get from a trackpad or Magic Mouse.

- **Works with Logitech mice** — detects Logi Options+ continuous events via scroll/momentum phase detection
- **Trackpad scrolling is left untouched** — real trackpad events (with non-zero phase) pass through unmodified
- **Works system-wide** across all apps
- **Instant start** — the first wheel tick from rest moves on the very next display frame
- **Self-healing** — if macOS disables the event tap (slow callback or sustained input), it is re-enabled automatically instead of silently reverting to choppy scrolling

### 2. Global shortcuts

Keyboard shortcuts that work in every app, configured from the settings window.

- **Open / toggle an app** — brings the app to the front; with **Toggle** on, pressing the key while that app is already in front hides it, so one key gets you in and out
- **Taken-combo detection** — before a shortcut is saved it is trial-registered, so the UI tells you whether the combination is *Available*, *Already assigned* in Smooth Scroll, *Held by another app or macOS*, or invalid. macOS refuses to register a hotkey another process owns, and that refusal is the only reliable signal there is
- **Safe recorder** — click the key box and press a combination. At least one modifier (⌥ ⌃ ⇧ ⌘) is required, so a shortcut can never swallow ordinary typing. Escape cancels
- **No extra permission** — shortcuts use the system hotkey API, not the event tap

### 3. Window snapping

Move and resize the frontmost app's focused window with a key, without installing Rectangle or Magnet.

| Action | What it does |
|--------|--------------|
| Left half / Right half | Fill the left or right half of the window's current display |
| Maximize | Fill the display's visible area (menu bar and Dock excluded). Not macOS full-screen |
| Centre | Centre the window on its display, keeping its size |
| Next display | Move to the next display, keeping position and size proportional |

The display used is the one that contains the window's centre, so it works naturally across multiple monitors.

### 4. Menu bar app

- **Menu bar only** — no Dock icon
- **Close = hide** — closing the settings window hides it; quit from the tray menu
- **Launch at Login** — toggle from the tray menu
- **On/off switch** for smooth scrolling in the tray menu and the settings window (shortcuts and window actions keep working either way)

## Default Shortcuts

Seeded on first run. Every one can be removed or rebound.

| Keys | Action |
|------|--------|
| ⌥S | Slack (toggle) |
| ⌥T | Ghostty (toggle) |
| ⌃⌥← | Window: left half |
| ⌃⌥→ | Window: right half |
| ⌃⌥↑ | Window: maximize |
| ⌃⌥↓ | Window: centre |
| ⌃⌥⌘→ | Window: next display |

Window actions follow Rectangle's ⌃⌥ convention. ⌥⌘-arrows were avoided on purpose: Ghostty uses them for split navigation and browsers for tab switching, and a global hotkey would silently break both.

Note: ⌥-letter combos normally type special characters (⌥S = ß, ⌥T = †). A global shortcut takes those over system-wide. Use ⌃⌥ or ⌃⌥⌘ combinations if you need the characters.

## Settings

| Setting | Range | Description |
|---------|-------|-------------|
| Scroll Speed | 0.5x – 10x | Impulse multiplier per wheel tick |
| Acceleration | 0 – 1 | Extra speed boost for fast scroll gestures |
| Glide | 0.80 – 0.99 | Velocity decay per frame — higher = more momentum |

Everything persists in `~/Library/Application Support/com.smoothscroll.macos/`:

| File | Contents |
|------|----------|
| `settings.json` | Scroll speed, acceleration, glide, on/off |
| `shortcuts.json` | Your shortcut list (keys + action) |
| `shortcuts.seed` | Which generation of defaults has been seeded, so new defaults appear once without re-adding ones you removed |

## Known Limitations

- **Full-screen windows cannot be snapped.** macOS does not let the Accessibility API move or resize a window in full-screen mode. The action is logged and the window is left alone — the same limit Rectangle has.
- **Fixed-size windows only move.** Apps that refuse resizing (some settings panels) get the position change but keep their size.
- **Shortcuts inside other apps are not detected.** The taken-combo check sees hotkeys held globally by other processes and by macOS. It cannot see a shortcut that only exists in a specific app's menus (for example ⌥⌘← in Ghostty).
- **Reduce Motion makes scrolling look stepped.** Synthetic scroll events are currently emitted in line units, so part of the smooth feel comes from macOS's own scroll animation; with Accessibility → Reduce Motion on, that animation is off. Moving to pixel-unit events is planned.
- **macOS only.**

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                         Tauri v2 App                          │
│                                                               │
│  ┌──────────────┐     Tauri Commands      ┌───────────────┐  │
│  │  Webview UI   │ ◄───────────────────► │  Rust Backend  │  │
│  │  (Settings)   │  settings, shortcuts   │                │  │
│  │  HTML/CSS/TS  │  check/add/remove      │                │  │
│  └──────────────┘                         └───┬───────┬───┘  │
│                                               │       │       │
│  ┌────────────────────────────────────────────┘       │       │
│  │  SCROLL                                            │       │
│  │  ┌─────────────────┐    ┌──────────────────────┐   │       │
│  │  │  CGEventTap      │    │  CVDisplayLink       │   │       │
│  │  │  (HID level)     │───►│  (display-synced)    │   │       │
│  │  │  intercepts raw  │    │  emits velocity-     │   │       │
│  │  │  wheel events    │    │  based scroll events │   │       │
│  │  └─────────────────┘    └──────────────────────┘   │       │
│  │                                                     │       │
│  │  SHORTCUTS                                          │       │
│  │  ┌─────────────────┐    ┌──────────────────────┐   │       │
│  │  │  Carbon hotkeys  │───►│  App action:         │◄──┘       │
│  │  │  (global-hotkey) │    │   NSWorkspace focus  │           │
│  │  │  fire on main    │    │   / hide             │           │
│  │  │  thread          │    │  Window action:      │           │
│  │  │                  │    │   AXUIElement move   │           │
│  │  │                  │    │   + resize           │           │
│  │  └─────────────────┘    └──────────────────────┘           │
│  └────────────────────────────────────────────────────────────┘
└──────────────────────────────────────────────────────────────┘
```

### Key Design Decisions — scrolling

**Velocity-Based Scroll Model** — Each mouse wheel tick adds an impulse to a velocity accumulator. Every display frame emits `velocity` pixels of scroll, then decays velocity. This produces smooth, consistent movement like a native macOS trackpad — no choppy position-animation stepping.

**Logi Options+ Compatibility** — Logi Options+ converts discrete wheel ticks into continuous pixel events (`isContinuous=1`). We distinguish these from real trackpad events by checking `scrollPhase` and `momentumPhase` — Logi events have both at 0, while real trackpad events have non-zero phases. Pixel deltas are read from `PointDelta` fields (96/97) instead of the integer `Delta` fields (11/12).

**Display-Synced Animation** — Uses `CVDisplayLink` instead of `thread::sleep`. The callback fires on a high-priority CoreVideo thread perfectly synced to the monitor's refresh rate (60Hz, 120Hz ProMotion, or variable). Falls back to a 120fps sleep loop if CVDisplayLink fails.

**Frame-Rate Independence** — Decay is calculated using `mach_absolute_time` to measure real elapsed time between frames, then adjusting the decay exponent accordingly. Scrolling feels identical on 60Hz, 120Hz, and variable refresh displays.

**Infinite Loop Prevention** — Two layers:
1. Synthetic events are posted to `kCGSessionEventTap` (downstream of our HID-level tap, so they never reach our callback)
2. Events are tagged with a sentinel value via `eventSourceUserData` (field 42) as a belt-and-suspenders guard

**Tap Recovery** — macOS silently disables an event tap when its callback is too slow (`kCGEventTapDisabledByTimeout`) or after sustained user input (`kCGEventTapDisabledByUserInput`). Both are caught and the tap is re-enabled, so scrolling never quietly reverts to choppy until a restart.

**Priority Inversion Avoidance** — Uses `std::sync::Mutex` (not `parking_lot`) because macOS's `pthread_mutex` supports priority inheritance. The event tap callback runs on a high-priority thread, so `parking_lot`'s userspace spinlock would cause priority inversion.

**Settings load before the app is built** — persisted settings are read before `tauri::Builder` runs, so the engine and the settings window both start from the saved values. Loading later in `setup` raced the webview's first request for settings.

**Scroll Physics**:
- *Instant start* — the first tick from rest goes straight into velocity so the very next display frame moves; ramping it added ~33ms (120Hz) / ~67ms (60Hz) of dead time before every scroll
- *Impulse ramp* — further ticks while already moving are fed into velocity over 4 frames for smooth continuation
- *Two-phase decay* — faster decay at high velocity, slower glide tail for natural feel
- *Direction reversal* — opposite momentum is killed instantly for responsive direction changes
- *Per-frame pixel cap* — time-based (4800px/s) to prevent jarring jumps, consistent across refresh rates
- *Sub-pixel precision* — fractional remainders carried between frames to prevent drift

### Key Design Decisions — shortcuts and windows

**Carbon hotkeys, not the event tap** — `tauri-plugin-global-shortcut` registers combos with `RegisterEventHotKey`. They fire in every app, need no extra permission, and are dispatched on the main thread, which is what AppKit's screen APIs require anyway.

**Trial registration as the availability check** — the only way to know whether another process holds a combo is to try to register it. The check registers, reads the result, and unregisters immediately.

**App focus via NSWorkspace** — the frontmost app is matched on its bundle file name (`Slack.app`), which is stable across localisation; hiding uses `NSRunningApplication.hide`. Bringing an app forward is plain `open -a`, which also launches it if it is not running.

**Window moves via the Accessibility API** — `AXUIElementCreateApplication(pid)` → `AXFocusedWindow` → set `AXPosition` and `AXSize`. Screen rectangles come from `NSScreen.visibleFrame` (bottom-left origin) and are flipped to AX's top-left origin around the primary display's height. Attributes are set position → size → position, because some apps clamp to a minimum size and shift, and others refuse a position that is off-screen for the *current* size.

**Default seeding by generation** — `shortcuts.seed` records which generation of defaults has been merged into the user's list. New defaults appear once for existing users; a default the user deleted is never re-added.

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

If you rebuild and reinstall often, sign with a stable identity so the Accessibility grant survives reinstalls (macOS ties the grant to the code signature):

```bash
APPLE_SIGNING_IDENTITY="Apple Development: Your Name (TEAMID)" npm run tauri build
```

If a reinstall ever leaves the app saying "Needs permission" while the toggle in System Settings still shows on, reset the stale entry and relaunch:

```bash
tccutil reset Accessibility com.smoothscroll.macos
```

## Requirements

- macOS 10.15+
- **Accessibility permission** — required for `CGEventTap` to intercept scroll events and for window snapping. The app prompts you to grant this on first launch. Global shortcuts need nothing extra.

## Development

```bash
# Install dependencies
npm install

# Run in development mode
npm run tauri dev

# Build release .app and .dmg
npm run tauri build
```

Debug builds write a log to `/tmp/smooth-scroll-debug.log` (the `dbg_log!` macro); it is compiled out of release builds.

### Project Structure

```
src-tauri/src/
├── lib.rs            # Tauri commands, tray menu, app lifecycle, settings persistence
├── logging.rs        # Shared debug logging (dbg_log! macro)
├── main.rs           # Entry point
├── shortcuts.rs      # Shortcut model, persistence, registration, availability check, app actions
├── window.rs         # Window snapping via the Accessibility API + NSScreen geometry
└── scroll/
    ├── mod.rs        # Public API, accessibility helpers
    ├── ffi.rs        # CoreGraphics/CoreVideo/mach FFI bindings
    ├── state.rs      # ScrollSettings (+ load/save), ScrollState, physics constants
    ├── physics.rs    # Frame processing, event callbacks
    └── engine.rs     # ScrollEngine lifecycle, thread management

src/
├── main.ts           # Settings UI: sliders, status polling, shortcut recorder and list
└── styles.css
index.html
```

### Stack

- **Frontend**: Vanilla TypeScript + CSS (no framework)
- **Backend**: Rust with raw CoreGraphics/CoreVideo/ApplicationServices FFI, `objc2-app-kit` for NSWorkspace/NSScreen
- **Framework**: Tauri v2 (`tauri-plugin-global-shortcut`, `tauri-plugin-autostart`, `tauri-plugin-opener`)
- **Build**: Vite + Cargo
- **CI**: GitHub Actions — builds DMG on release

## License

MIT
