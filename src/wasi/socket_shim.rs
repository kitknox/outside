//! Thin Rust bindings over the `rootshell_socket_*` ABI exposed by the
//! rootshell WASM host. All ints are i32, all pointers are u32 offsets
//! into WASM linear memory.

#![allow(dead_code)]

pub const AF_INET: i32 = 2;
pub const AF_INET6: i32 = 30;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;

unsafe extern "C" {
    pub unsafe fn rootshell_socket_socket(domain: i32, type_: i32, fd_out: *mut i32) -> i32;
    pub unsafe fn rootshell_socket_bind(fd: i32, addr: *const u8, addrlen: i32) -> i32;
    pub unsafe fn rootshell_socket_listen(fd: i32, backlog: i32) -> i32;
    pub unsafe fn rootshell_socket_accept(fd: i32, fd_out: *mut i32) -> i32;
    pub unsafe fn rootshell_socket_connect(fd: i32, addr: *const u8, addrlen: i32) -> i32;
    pub unsafe fn rootshell_socket_send(fd: i32, buf: *const u8, len: i32, sent_out: *mut u32) -> i32;
    pub unsafe fn rootshell_socket_recv(fd: i32, buf: *mut u8, len: i32, recv_out: *mut u32) -> i32;
    pub unsafe fn rootshell_socket_shutdown(fd: i32, how: i32) -> i32;
    pub unsafe fn rootshell_socket_close(fd: i32) -> i32;

    // Hostname-friendly variants — DNS is delegated to Network.framework
    // on the host side, so we never need a sockaddr round-trip.
    pub unsafe fn rootshell_socket_connect_host(
        fd: i32, host: *const u8, host_len: i32, port: u16,
    ) -> i32;
    // TLS: host wraps the underlying NWConnection in NWParameters(tls:tcp:),
    // doing the handshake + cert validation outside the sandbox. After this
    // returns 0, regular send/recv operate on the plaintext stream.
    pub unsafe fn rootshell_socket_tls_connect_host(
        fd: i32, host: *const u8, host_len: i32, port: u16,
    ) -> i32;
}

unsafe extern "C" {
    pub unsafe fn rootshell_terminal_set_raw(enabled: i32) -> i32;
    pub unsafe fn rootshell_terminal_is_tty(fd: i32) -> i32;
}

pub fn errno_name(e: i32) -> &'static str {
    match e {
        0 => "OK",
        2 => "EACCES",
        8 => "EBADF",
        14 => "ECONNREFUSED",
        27 => "EINTR",
        28 => "EINVAL",
        50 => "ENETUNREACH",
        73 => "ETIMEDOUT",
        _ => "errno",
    }
}

pub fn io_err(errno: i32) -> std::io::Error {
    std::io::Error::other(format!("rootshell_socket: {} ({})", errno_name(errno), errno))
}
