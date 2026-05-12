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

/// Read terminal dimensions from the env vars the host sets at process
/// launch. There is no SIGWINCH equivalent — these are a one-shot snapshot.
pub fn screen_size() -> (u16, u16) {
    let cols = std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(80);
    let rows = std::env::var("LINES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(24);
    (cols, rows)
}
