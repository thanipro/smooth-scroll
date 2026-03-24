# Smooth Scroll

A macOS utility that adds smooth, fluid scrolling to any mouse — built with **Tauri v2** and **Rust**.

## The Story

I was about to renew my SmoothScroll app subscription — $12/year just to make my mouse scroll smoothly. Then I thought: why am I paying for this? So I sat down with Claude and built my own in about an hour.

I didn't write the code myself. I described what I wanted, asked Claude to build it, reviewed the output, asked it to research performance issues, ran code reviews, and iterated until it was solid. The whole process took roughly one hour from idea to a working `.app` bundle.

At least I don't have to pay for a scroll utility subscription anymore.

## What It Does

Intercepts the choppy, discrete scroll events from a regular mouse and replaces them with smooth, animated pixel-based scrolling — the same feel you get from a trackpad or Magic Mouse.

- Trackpad scrolling is left untouched
- Works system-wide across all apps
- Runs as a lightweight menu bar utility (~5MB)

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
│  │  │  Intercepts raw  │───►│  Emits smooth pixel  │  │
│  │  │  scroll events   │    │  scroll events at    │  │
│  │  │  from mouse      │    │  monitor refresh     │  │
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

**Scroll Interception** — Uses `CGEventTap` at the HID level via raw CoreGraphics FFI. The Rust `core-graphics` crate doesn't expose the event tap API, so we call `CGEventTapCreate`, `CGEventPost`, etc. directly through `extern "C"` bindings.

**Display-Synced Animation** — Uses `CVDisplayLink` instead of `thread::sleep`. The callback fires on a high-priority CoreVideo thread perfectly synced to the monitor's refresh rate (60Hz, 120Hz ProMotion, or variable). Falls back to a 120fps sleep loop if CVDisplayLink fails.

**Infinite Loop Prevention** — Two layers:
1. Synthetic events are posted to `kCGSessionEventTap` (downstream of our HID-level tap, so they never reach our callback)
2. Events are tagged with a sentinel value via `eventSourceUserData` (field 42) as a belt-and-suspenders guard

**Priority Inversion Avoidance** — Uses `std::sync::Mutex` (not `parking_lot`) because macOS's `pthread_mutex` supports priority inheritance. The event tap callback runs on a high-priority thread, so `parking_lot`'s userspace spinlock would cause priority inversion.

**Animation Accumulation** — When a new scroll event arrives while an animation is in progress, the remaining un-emitted distance from the old animation is carried forward into the new one. This prevents scroll distance loss during rapid scrolling.

**Sub-Pixel Precision** — Fractional pixel remainders from `f64 → i32` truncation are accumulated and carried between frames, preventing drift over long animations.

## Settings

| Setting | Range | Description |
|---------|-------|-------------|
| Scroll Speed | 0.5x – 10x | Multiplier applied to scroll distance |
| Acceleration | 0 – 1 | Extra speed boost for fast scroll gestures |
| Smoothness | 50ms – 800ms | Animation duration per scroll step |
| Momentum | On/Off | Enable inertia-style glide after scrolling |
| Glide Distance | 0.80 – 0.99 | How far momentum carries (when enabled) |
| Easing Curve | Ease Out / Ease In-Out / Linear | Animation shape |

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

### Stack

- **Frontend**: Vanilla TypeScript + CSS (no framework)
- **Backend**: Rust with raw CoreGraphics/CoreVideo FFI
- **Framework**: Tauri v2
- **Build**: Vite + Cargo

## License

MIT
