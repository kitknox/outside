//! Sync HTTP/1.1 GET client built on top of rootshell sockets. Public surface
//! mirrors `api::client::{get, get_with_retry}` so the call sites in
//! `api::weather`, `api::geolocation`, and `api::iplocation` work unchanged.
//!
//! Why hand-rolled rather than a crate: `reqwest`/`ureq` pull in tokio or
//! native-tls or rustls, none of which compile to `wasm32-wasip1`. The TLS
//! layer here is delegated entirely to the host (see `wasi::stream::TlsStream`).
//!
//! Framing strategy: we always send `Connection: close` so the server closes
//! the socket at body end. That lets us read-to-EOF and skip chunked decoding
//! entirely. All three target APIs (open-meteo, geocoding, ip-api) honor it.

use std::io::{Read, Write};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use url::Url;

use super::stream::{PlainStream, TlsStream};

const READ_CHUNK: usize = 4096;
const MAX_RESPONSE: usize = 4 * 1024 * 1024; // 4 MiB cap — the APIs return <100KB
const USER_AGENT: &str = concat!("outside/", env!("CARGO_PKG_VERSION"), " (wasi)");

/// Perform a synchronous HTTP/1.1 GET and return the body as a String.
pub fn get(url: &str) -> Result<String> {
    let parsed = Url::parse(url).with_context(|| format!("Invalid URL: {url}"))?;

    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("URL has no host: {url}"))?
        .to_string();
    let scheme = parsed.scheme();
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| anyhow!("URL has no port and scheme has no default: {url}"))?;

    // path?query — open-meteo always has a query string, so we rebuild it
    // rather than relying on Url's display which would include the scheme.
    let mut path = parsed.path().to_string();
    if path.is_empty() {
        path.push('/');
    }
    if let Some(q) = parsed.query() {
        path.push('?');
        path.push_str(q);
    }

    let body = match scheme {
        "https" => {
            let stream = TlsStream::connect(&host, port)
                .with_context(|| format!("Unable to TLS-connect to {host}:{port}"))?;
            do_request(stream, &host, &path)?
        }
        "http" => {
            let stream = PlainStream::connect(&host, port)
                .with_context(|| format!("Unable to connect to {host}:{port}"))?;
            do_request(stream, &host, &path)?
        }
        other => bail!("Unsupported URL scheme: {other}"),
    };

    Ok(body)
}

/// `get` with exponential-backoff retry, matching the native isahc client's
/// API. Retries up to `max_retries` times after the initial attempt.
///
/// Sleep here uses our busy-wait helper rather than `std::thread::sleep` —
/// some rootshell builds return ENOSYS from `poll_oneoff`, which makes std
/// panic. The retry delay is short (≤400ms total across 2 retries) so the
/// CPU cost of spinning is negligible. Streaming intervals (in main.rs) use
/// std::thread::sleep instead since those are long-running.
pub fn get_with_retry(url: &str, max_retries: usize) -> Result<String> {
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 0..=max_retries {
        match get(url) {
            Ok(body) => return Ok(body),
            Err(e) => {
                // Surface intermediate errors. The native isahc client
                // silently swallowed these; under WASI it's far more useful
                // to see why retries were needed (TLS handshake fail vs.
                // DNS vs. HTTP 5xx, etc.).
                if attempt < max_retries {
                    eprintln!("outside: attempt {} failed: {}; retrying", attempt + 1, e);
                }
                last_error = Some(e);
                if attempt < max_retries {
                    let delay = Duration::from_millis(100 * (2_u64.pow(attempt as u32)));
                    crate::wasi::terminal::sleep_busy(delay);
                }
            }
        }
    }

    Err(last_error.unwrap())
}

fn do_request<S: Read + Write>(mut stream: S, host: &str, path: &str) -> Result<String> {
    // Accept-Encoding: identity defends against CDN-injected gzip — we don't
    // have a decompressor handy and the response is small enough that the
    // bandwidth saving is irrelevant.
    let req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         User-Agent: {ua}\r\n\
         Accept: application/json\r\n\
         Accept-Encoding: identity\r\n\
         Connection: close\r\n\
         \r\n",
        path = path,
        host = host,
        ua = USER_AGENT,
    );
    stream
        .write_all(req.as_bytes())
        .with_context(|| "Unable to send HTTP request")?;

    // Read to EOF. The 4 MiB cap prevents accidental OOM if a server misbehaves.
    let mut buf = Vec::with_capacity(8192);
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        let n = stream
            .read(&mut chunk)
            .with_context(|| "Unable to read HTTP response")?;
        if n == 0 {
            break;
        }
        if buf.len() + n > MAX_RESPONSE {
            bail!("HTTP response exceeded {} bytes", MAX_RESPONSE);
        }
        buf.extend_from_slice(&chunk[..n]);
    }

    // Parse the head so we can check status and inspect framing headers.
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut resp = httparse::Response::new(&mut headers);
    let head_end = match resp
        .parse(&buf)
        .with_context(|| "Malformed HTTP response head")?
    {
        httparse::Status::Complete(n) => n,
        httparse::Status::Partial => bail!("Truncated HTTP response head"),
    };

    let status = resp.code.unwrap_or(0);
    if !(200..400).contains(&status) {
        bail!("HTTP request failed with status {status}");
    }

    // CDNs (open-meteo's included) routinely use Transfer-Encoding: chunked
    // even when the request says Connection: close. Detect and decode.
    let is_chunked = resp.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("transfer-encoding")
            && std::str::from_utf8(h.value)
                .map(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("chunked")))
                .unwrap_or(false)
    });

    let raw_body = &buf[head_end..];
    let body = if is_chunked {
        decode_chunked(raw_body)?
    } else {
        raw_body.to_vec()
    };

    String::from_utf8(body).with_context(|| "HTTP response body is not valid UTF-8")
}

/// Decode HTTP/1.1 chunked transfer encoding (RFC 7230 §4.1).
///
/// Format per chunk: `<hex size>[;chunk-ext]\r\n<size bytes of data>\r\n`.
/// Stream ends with a zero-size chunk followed by optional trailers and a
/// final CRLF. We ignore trailers — none of the target APIs send any.
fn decode_chunked(input: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len());
    let mut pos = 0;

    loop {
        // Locate CRLF at end of the chunk-size line.
        let rel = input[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| anyhow!("Chunked: no CRLF after chunk size"))?;
        let size_line = &input[pos..pos + rel];

        // Strip chunk-extensions (everything after the first ';').
        let size_str = std::str::from_utf8(size_line)
            .map_err(|_| anyhow!("Chunked: non-utf8 chunk size"))?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();

        let size = usize::from_str_radix(size_str, 16)
            .map_err(|_| anyhow!("Chunked: invalid chunk size {:?}", size_str))?;

        pos += rel + 2; // past size line + CRLF

        if size == 0 {
            // Final chunk reached. Caller doesn't need trailers.
            return Ok(out);
        }

        if pos + size > input.len() {
            bail!("Chunked: body truncated (want {} bytes, have {})", size, input.len() - pos);
        }
        out.extend_from_slice(&input[pos..pos + size]);
        pos += size;

        // Each chunk's data is followed by CRLF.
        if pos + 2 > input.len() || &input[pos..pos + 2] != b"\r\n" {
            bail!("Chunked: missing CRLF after chunk data");
        }
        pos += 2;
    }
}
