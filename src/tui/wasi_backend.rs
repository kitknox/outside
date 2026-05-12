//! Custom `cursive::backend::Backend` implementation for the rootshell WASI
//! host. Replaces the crossterm backend, which doesn't compile on
//! `wasm32-wasip1` (it depends on signal_hook, mio, raw termios, etc).
//!
//! Strategy:
//!   * Output: ANSI escape sequences written to stdout. iOS rootshell's
//!     terminal supports 256-color, truecolor, alt-screen, and standard SGR.
//!   * Input: byte-at-a-time blocking reads from stdin (raw mode flipped on
//!     during init via `rootshell_terminal_set_raw(1)`). A small state machine
//!     decodes CSI/SS3 escape sequences into `cursive::event::Event`s.
//!   * Size: read once from `COLUMNS` / `LINES` env vars (no SIGWINCH).
//!
//! Cursive's docstring says `poll_event` "should return immediately" with
//! `None` when no input is available. WASI fd_read on stdin is purely
//! blocking — there's no `poll` equivalent. So poll_event here blocks until
//! a key arrives. The UI therefore only repaints in response to user input,
//! which matches an event-driven app's expectations. Auto-refresh of the
//! cache_age progress bar is unavailable under WASI (see async_operations).

use std::cell::{Cell, RefCell};
use std::io::{self, BufWriter, Read, Write};

use cursive::backend::Backend;
use cursive::event::{Event, Key};
use cursive::theme::{BaseColor, Color, ColorPair, Effect};
use cursive::Vec2;

use crate::wasi::terminal;

pub struct WasiBackend {
    stdout: RefCell<BufWriter<io::Stdout>>,
    size: Vec2,
    current_style: Cell<ColorPair>,
    /// Cursive's `process_events` is `while let Some(ev) = backend.poll_event()`
    /// — it drains all pending events before drawing. With a blocking stdin
    /// read we'd starve the draw step forever. We alternate: one block-read
    /// returns `Some(event)`, the next call returns `None` so cursive can
    /// refresh, the call after that blocks again. Crucially, because at
    /// least one event was returned this frame, cursive sets `boring=false`
    /// and skips the 30ms `thread::sleep` in `post_events` — important on
    /// hosts that don't yet implement `poll_oneoff` for clock events.
    drained_this_frame: Cell<bool>,
}

impl WasiBackend {
    pub fn init() -> Box<dyn Backend> {
        terminal::set_raw(true);

        let (cols, rows) = terminal::screen_size();
        let size = Vec2::new(cols as usize, rows as usize);

        let mut stdout = BufWriter::new(io::stdout());
        // Alt screen + hide cursor + clear + home cursor. Written eagerly so
        // the user sees the TUI take over the terminal even if the first
        // refresh() is delayed by a fetch.
        let _ = stdout.write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H");
        let _ = stdout.flush();

        Box::new(Self {
            stdout: RefCell::new(stdout),
            size,
            current_style: Cell::new(ColorPair {
                front: Color::TerminalDefault,
                back: Color::TerminalDefault,
            }),
            drained_this_frame: Cell::new(false),
        })
    }

    fn write_raw(&self, s: &str) {
        let _ = self.stdout.borrow_mut().write_all(s.as_bytes());
    }

    fn write_bytes(&self, b: &[u8]) {
        let _ = self.stdout.borrow_mut().write_all(b);
    }
}

impl Drop for WasiBackend {
    fn drop(&mut self) {
        // Reset SGR, show cursor, leave alt screen.
        let _ = self
            .stdout
            .borrow_mut()
            .write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = self.stdout.borrow_mut().flush();
        terminal::set_raw(false);
    }
}

impl Backend for WasiBackend {
    fn name(&self) -> &str {
        "wasi"
    }

    // Tells cursive to do delta updates rather than redraw everything every
    // frame. Critical for performance over the JS→Swift→stdout pipe.
    fn is_persistent(&self) -> bool {
        true
    }

    fn set_title(&mut self, _title: String) {
        // No-op: the rootshell WASM host doesn't expose a window-title API,
        // and the inner terminal's title is owned by the host shell anyway.
    }

    fn refresh(&mut self) {
        let _ = self.stdout.borrow_mut().flush();
    }

    fn has_colors(&self) -> bool {
        true
    }

    fn screen_size(&self) -> Vec2 {
        self.size
    }

    fn move_to(&self, pos: Vec2) {
        // ANSI CUP is 1-indexed, cursive is 0-indexed.
        let s = format!("\x1b[{};{}H", pos.y + 1, pos.x + 1);
        self.write_raw(&s);
    }

    fn print(&self, text: &str) {
        self.write_bytes(text.as_bytes());
    }

    fn clear(&self, color: Color) {
        let pair = ColorPair { front: color, back: color };
        self.apply_colors(pair);
        self.write_raw("\x1b[2J\x1b[H");
    }

    fn set_color(&self, colors: ColorPair) -> ColorPair {
        let prev = self.current_style.get();
        if prev != colors {
            self.apply_colors(colors);
            self.current_style.set(colors);
        }
        prev
    }

    fn set_effect(&self, effect: Effect) {
        match effect {
            Effect::Simple => {}
            Effect::Reverse => self.write_raw("\x1b[7m"),
            Effect::Bold => self.write_raw("\x1b[1m"),
            Effect::Dim => self.write_raw("\x1b[2m"),
            Effect::Italic => self.write_raw("\x1b[3m"),
            Effect::Underline => self.write_raw("\x1b[4m"),
            Effect::Blink => self.write_raw("\x1b[5m"),
            Effect::Strikethrough => self.write_raw("\x1b[9m"),
        }
    }

    fn unset_effect(&self, effect: Effect) {
        match effect {
            Effect::Simple => {}
            Effect::Reverse => self.write_raw("\x1b[27m"),
            Effect::Bold | Effect::Dim => self.write_raw("\x1b[22m"),
            Effect::Italic => self.write_raw("\x1b[23m"),
            Effect::Underline => self.write_raw("\x1b[24m"),
            Effect::Blink => self.write_raw("\x1b[25m"),
            Effect::Strikethrough => self.write_raw("\x1b[29m"),
        }
    }

    fn poll_event(&mut self) -> Option<Event> {
        // See `drained_this_frame` for why we alternate. First call of a
        // frame: block-read one event from stdin, mark drained, return it.
        // Cursive will call us again immediately — second call: clear the
        // flag and return None, which makes cursive break out of its event-
        // drain loop and call refresh() to actually paint the screen.
        if self.drained_this_frame.get() {
            self.drained_this_frame.set(false);
            return None;
        }
        let ev = read_event();
        if ev.is_some() {
            self.drained_this_frame.set(true);
        }
        ev
    }
}

impl WasiBackend {
    fn apply_colors(&self, pair: ColorPair) {
        let mut sgr = String::from("\x1b[");
        sgr.push_str(&sgr_fg(pair.front));
        sgr.push(';');
        sgr.push_str(&sgr_bg(pair.back));
        sgr.push('m');
        self.write_raw(&sgr);
    }
}

fn sgr_fg(c: Color) -> String {
    match c {
        Color::TerminalDefault => "39".into(),
        Color::Dark(b) => format!("{}", 30 + base_color_index(b)),
        Color::Light(b) => format!("{}", 90 + base_color_index(b)),
        Color::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),
        Color::RgbLowRes(r, g, b) => format!("38;5;{}", 16 + 36 * r + 6 * g + b),
    }
}

fn sgr_bg(c: Color) -> String {
    match c {
        Color::TerminalDefault => "49".into(),
        Color::Dark(b) => format!("{}", 40 + base_color_index(b)),
        Color::Light(b) => format!("{}", 100 + base_color_index(b)),
        Color::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),
        Color::RgbLowRes(r, g, b) => format!("48;5;{}", 16 + 36 * r + 6 * g + b),
    }
}

fn base_color_index(b: BaseColor) -> u8 {
    match b {
        BaseColor::Black => 0,
        BaseColor::Red => 1,
        BaseColor::Green => 2,
        BaseColor::Yellow => 3,
        BaseColor::Blue => 4,
        BaseColor::Magenta => 5,
        BaseColor::Cyan => 6,
        BaseColor::White => 7,
    }
}

// ---------------- Input parsing ----------------

/// Read one byte from stdin (blocking). Returns None on EOF.
fn read_byte() -> Option<u8> {
    let mut buf = [0u8; 1];
    match io::stdin().read(&mut buf) {
        Ok(0) => None,
        Ok(_) => Some(buf[0]),
        Err(_) => None,
    }
}

/// Decode one logical key event. CSI/SS3 sequences are consumed inline; if
/// the host delivers them in fragments across multiple fd_read returns this
/// still works because each call simply blocks for the next byte.
fn read_event() -> Option<Event> {
    let b = read_byte()?;

    match b {
        0x1b => read_after_esc(),
        0x7f => Some(Event::Key(Key::Backspace)),
        0x0d | 0x0a => Some(Event::Key(Key::Enter)),
        0x09 => Some(Event::Key(Key::Tab)),
        // Ctrl-C in raw mode arrives as a stdin byte rather than killing
        // the process. Map it to CtrlChar('c') so cursive's quit handler
        // (if any) can react.
        0x03 => Some(Event::CtrlChar('c')),
        0x01..=0x1a => Some(Event::CtrlChar((b - 1 + b'a') as char)),
        0x20..=0x7e => Some(Event::Char(b as char)),
        // UTF-8 leading byte — collect continuation bytes.
        0xc0..=0xff => {
            let extra = utf8_continuation_count(b)?;
            let mut bytes = vec![b];
            for _ in 0..extra {
                bytes.push(read_byte()?);
            }
            std::str::from_utf8(&bytes)
                .ok()
                .and_then(|s| s.chars().next())
                .map(Event::Char)
        }
        _ => None,
    }
}

fn utf8_continuation_count(lead: u8) -> Option<usize> {
    if lead & 0b1110_0000 == 0b1100_0000 {
        Some(1)
    } else if lead & 0b1111_0000 == 0b1110_0000 {
        Some(2)
    } else if lead & 0b1111_1000 == 0b1111_0000 {
        Some(3)
    } else {
        None
    }
}

/// Already consumed 0x1b. Read the next byte and dispatch:
///   '[' → CSI sequence
///   'O' → SS3 (F1–F4)
///   0x1b → bare Esc (followed by Esc — emit Esc, the next iteration will
///          handle the second one)
///   anything else → Alt+<char>, which cursive expresses as AltChar.
fn read_after_esc() -> Option<Event> {
    let next = read_byte()?;
    match next {
        b'[' => read_csi(),
        b'O' => read_ss3(),
        0x1b => Some(Event::Key(Key::Esc)),
        // Alt-prefixed key.
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => Some(Event::AltChar(next as char)),
        _ => Some(Event::Key(Key::Esc)),
    }
}

fn read_csi() -> Option<Event> {
    // CSI parameters: digits + ';', terminated by a "final byte" 0x40..=0x7e.
    // We only need a small subset, so the parser is hand-coded.
    let first = read_byte()?;
    match first {
        b'A' => Some(Event::Key(Key::Up)),
        b'B' => Some(Event::Key(Key::Down)),
        b'C' => Some(Event::Key(Key::Right)),
        b'D' => Some(Event::Key(Key::Left)),
        b'H' => Some(Event::Key(Key::Home)),
        b'F' => Some(Event::Key(Key::End)),
        b'Z' => Some(Event::Shift(Key::Tab)), // BackTab — Shift+Tab
        b'0'..=b'9' => read_csi_with_params(first),
        _ => None,
    }
}

fn read_csi_with_params(first_digit: u8) -> Option<Event> {
    // Collect digits up to a non-digit final byte. Most apps emit one or two
    // numeric params; we cap collection at 6 bytes to stay defensive.
    let mut digits = vec![first_digit];
    let final_byte;
    loop {
        let b = read_byte()?;
        if b.is_ascii_digit() || b == b';' {
            if digits.len() < 6 {
                digits.push(b);
            }
        } else {
            final_byte = b;
            break;
        }
    }

    let param: u32 = std::str::from_utf8(&digits).ok()?.split(';').next()?.parse().ok()?;

    if final_byte == b'~' {
        match param {
            1 | 7 => Some(Event::Key(Key::Home)),
            2 => Some(Event::Key(Key::Ins)),
            3 => Some(Event::Key(Key::Del)),
            4 | 8 => Some(Event::Key(Key::End)),
            5 => Some(Event::Key(Key::PageUp)),
            6 => Some(Event::Key(Key::PageDown)),
            11 => Some(Event::Key(Key::F1)),
            12 => Some(Event::Key(Key::F2)),
            13 => Some(Event::Key(Key::F3)),
            14 => Some(Event::Key(Key::F4)),
            15 => Some(Event::Key(Key::F5)),
            17 => Some(Event::Key(Key::F6)),
            18 => Some(Event::Key(Key::F7)),
            19 => Some(Event::Key(Key::F8)),
            20 => Some(Event::Key(Key::F9)),
            21 => Some(Event::Key(Key::F10)),
            23 => Some(Event::Key(Key::F11)),
            24 => Some(Event::Key(Key::F12)),
            _ => None,
        }
    } else {
        None
    }
}

fn read_ss3() -> Option<Event> {
    match read_byte()? {
        b'P' => Some(Event::Key(Key::F1)),
        b'Q' => Some(Event::Key(Key::F2)),
        b'R' => Some(Event::Key(Key::F3)),
        b'S' => Some(Event::Key(Key::F4)),
        _ => None,
    }
}
