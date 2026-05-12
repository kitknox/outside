//! `Read + Write` wrappers over rootshell sockets. `PlainStream` is raw TCP;
//! `TlsStream` adds host-side TLS via `rootshell_socket_tls_connect_host`.
//! Both close their underlying fd on Drop.

use std::io::{self, Read, Write};

use super::socket_shim::*;

pub struct PlainStream {
    fd: i32,
}

pub struct TlsStream {
    fd: i32,
}

impl PlainStream {
    pub fn connect(host: &str, port: u16) -> io::Result<Self> {
        unsafe {
            let mut fd: i32 = -1;
            let e = rootshell_socket_socket(AF_INET, SOCK_STREAM, &mut fd);
            if e != 0 {
                return Err(io_err(e));
            }
            let e =
                rootshell_socket_connect_host(fd, host.as_ptr(), host.len() as i32, port);
            if e != 0 {
                let _ = rootshell_socket_close(fd);
                return Err(io_err(e));
            }
            Ok(Self { fd })
        }
    }
}

impl TlsStream {
    pub fn connect(host: &str, port: u16) -> io::Result<Self> {
        unsafe {
            let mut fd: i32 = -1;
            let e = rootshell_socket_socket(AF_INET, SOCK_STREAM, &mut fd);
            if e != 0 {
                return Err(io_err(e));
            }
            let e = rootshell_socket_tls_connect_host(
                fd,
                host.as_ptr(),
                host.len() as i32,
                port,
            );
            if e != 0 {
                let _ = rootshell_socket_close(fd);
                return Err(io_err(e));
            }
            Ok(Self { fd })
        }
    }
}

/// Loop send until the entire buffer is drained — `rootshell_socket_send`
/// can short-write the same way BSD `send(2)` can.
unsafe fn fd_write_all(fd: i32, buf: &[u8]) -> io::Result<()> {
    let mut sent_total = 0usize;
    while sent_total < buf.len() {
        let mut sent: u32 = 0;
        let remaining = &buf[sent_total..];
        let e = unsafe {
            rootshell_socket_send(
                fd,
                remaining.as_ptr(),
                remaining.len() as i32,
                &mut sent,
            )
        };
        if e != 0 {
            return Err(io_err(e));
        }
        if sent == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "rootshell_socket_send returned 0",
            ));
        }
        sent_total += sent as usize;
    }
    Ok(())
}

unsafe fn fd_read_once(fd: i32, buf: &mut [u8]) -> io::Result<usize> {
    let mut got: u32 = 0;
    let e = unsafe {
        rootshell_socket_recv(fd, buf.as_mut_ptr(), buf.len() as i32, &mut got)
    };
    if e != 0 {
        return Err(io_err(e));
    }
    Ok(got as usize)
}

impl Read for PlainStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        unsafe { fd_read_once(self.fd, buf) }
    }
}

impl Write for PlainStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        unsafe { fd_write_all(self.fd, buf)? };
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for TlsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        unsafe { fd_read_once(self.fd, buf) }
    }
}

impl Write for TlsStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        unsafe { fd_write_all(self.fd, buf)? };
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for PlainStream {
    fn drop(&mut self) {
        unsafe { rootshell_socket_close(self.fd) };
    }
}

impl Drop for TlsStream {
    fn drop(&mut self) {
        unsafe { rootshell_socket_close(self.fd) };
    }
}
