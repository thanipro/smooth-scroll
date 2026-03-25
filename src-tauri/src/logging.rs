//! Shared debug logging — writes to stderr AND `/tmp/smooth-scroll-debug.log`.
//! Active only in debug builds; completely compiled out in release.

#[cfg(debug_assertions)]
pub const LOG_PATH: &str = "/tmp/smooth-scroll-debug.log";

#[cfg(debug_assertions)]
use std::io::Write;
#[cfg(debug_assertions)]
use std::sync::Mutex;

#[cfg(debug_assertions)]
static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

#[cfg(debug_assertions)]
pub fn init() {
    if let Ok(mut guard) = LOG_FILE.lock() {
        if guard.is_none() {
            *guard = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(LOG_PATH)
                .ok();
        }
    }
}

#[cfg(debug_assertions)]
pub fn write(msg: &str) {
    eprintln!("{msg}");
    if let Ok(mut guard) = LOG_FILE.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "{msg}");
            let _ = f.flush();
        }
    }
}

/// Debug log macro — writes to both stderr and the shared log file.
/// Completely compiled out in release builds.
#[macro_export]
#[cfg(debug_assertions)]
macro_rules! dbg_log {
    ($($arg:tt)*) => {
        $crate::logging::write(&format!(
            "[SmoothScroll][{}:{}] {}",
            file!(), line!(), format!($($arg)*)
        ))
    };
}

#[macro_export]
#[cfg(not(debug_assertions))]
macro_rules! dbg_log {
    ($($arg:tt)*) => {};
}
