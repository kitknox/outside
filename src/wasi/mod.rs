//! WASI-specific glue: rootshell FFI, sync HTTP/1.1 over TLS, and terminal
//! helpers. Everything in here is cfg-gated to `target_os = "wasi"` at the
//! crate root so the native build never parses these files.

pub mod socket_shim;
pub mod stream;
pub mod http;
pub mod terminal;
