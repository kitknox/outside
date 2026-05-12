//! Safe wrappers over the host-side terminal control imports, plus a
//! sleep helper that doesn't rely on the WASI `poll_oneoff` import.

use super::socket_shim::rootshell_terminal_set_raw;

/// Busy-wait sleep. Falls back to spinning on `Instant::elapsed()` rather
/// than `std::thread::sleep` because some host shells return `ENOSYS` from
/// `poll_oneoff`, which makes std panic. Use this for short delays only
/// (retry backoff, ~100ms–500ms). For long sleeps (streaming intervals)
/// `std::thread::sleep` is fine once the host implements `poll_oneoff`
/// for `EVENTTYPE_CLOCK` (recent rootshell builds do — see wasm-worker.js).
pub fn sleep_busy(duration: std::time::Duration) {
    let start = std::time::Instant::now();
    while start.elapsed() < duration {
        // No-op spin. sched_yield exists in WASI but is a no-op in the JS
        // shim, so there's no point calling it.
    }
}

/// Toggle the host's stdin handling between raw and cooked modes.
///
/// Raw (`true`): every keystroke, including 0x03 (Ctrl-C), is delivered as
/// stdin bytes; the WASM app decides what to do.
///
/// Cooked (`false`, default): Ctrl-C cancels the WASM process at the host
/// layer (matches `tcsetattr(ICANON | ISIG)` semantics).
pub fn set_raw(enabled: bool) {
    unsafe {
        rootshell_terminal_set_raw(if enabled { 1 } else { 0 });
    }
}

use std::sync::atomic::{AtomicU16, Ordering};

static COLS: AtomicU16 = AtomicU16::new(0);
static ROWS: AtomicU16 = AtomicU16::new(0);

/// Current terminal dimensions. Lazy-initialised from `COLUMNS` / `LINES` at
/// process launch (the rootshell host sets both); updated thereafter by
/// `set_screen_size`, which the WasiBackend CSI parser calls when libghostty
/// sends a DEC mode 2048 size report (`CSI 48 ; rows ; cols ; py ; px t`).
pub fn screen_size() -> (u16, u16) {
    let mut c = COLS.load(Ordering::Relaxed);
    let mut r = ROWS.load(Ordering::Relaxed);
    if c == 0 || r == 0 {
        c = std::env::var("COLUMNS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(80);
        r = std::env::var("LINES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(24);
        COLS.store(c, Ordering::Relaxed);
        ROWS.store(r, Ordering::Relaxed);
    }
    (c, r)
}

pub fn set_screen_size(cols: u16, rows: u16) {
    COLS.store(cols, Ordering::Relaxed);
    ROWS.store(rows, Ordering::Relaxed);
}
