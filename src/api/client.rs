// Native and WASI use entirely different HTTP stacks. Native is isahc (libcurl
// + OpenSSL); WASI is our hand-rolled HTTP/1.1 over rootshell sockets with
// host-side TLS. The public surface — `get` and `get_with_retry` — is the
// same on both, so callers in `api::weather`, `api::geolocation`, and
// `api::iplocation` don't need to know which is in use.

#[cfg(not(target_os = "wasi"))]
mod native {
    use anyhow::{Context, Result};
    use isahc::config::Configurable;
    use isahc::{HttpClient, HttpClientBuilder, ReadResponseExt};
    use std::sync::OnceLock;
    use std::time::Duration;

    static HTTP_CLIENT: OnceLock<HttpClient> = OnceLock::new();

    /// Returns a shared HTTP client instance configured with appropriate timeouts and connection pooling.
    ///
    /// The client is created once and reused for all HTTP requests to improve performance.
    /// Configuration includes:
    /// - 2 second connection timeout
    /// - 5 second request timeout
    /// - 15 second TCP keepalive
    /// - Maximum 4 connections per host
    pub fn get_client() -> &'static HttpClient {
        HTTP_CLIENT.get_or_init(|| {
            HttpClientBuilder::new()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(5))
                .tcp_keepalive(Duration::from_secs(15))
                .max_connections_per_host(4)
                .build()
                .expect("Unable to create HTTP client")
        })
    }

    /// Performs a GET request to the specified URL and returns the response body as a string.
    pub fn get(url: &str) -> Result<String> {
        let client = get_client();

        let mut response =
            client.get(url).with_context(|| format!("Unable to send request to {url}"))?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "HTTP request failed with status: {} for URL: {}",
                response.status(),
                url
            ));
        }

        response.text().with_context(|| format!("Unable to read response body from {url}"))
    }

    /// Performs a GET request with exponential backoff retry logic.
    pub fn get_with_retry(url: &str, max_retries: usize) -> Result<String> {
        let mut last_error = None;

        for attempt in 0..=max_retries {
            match get(url) {
                Ok(response) => return Ok(response),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        let delay = Duration::from_millis(100 * (2_u64.pow(attempt as u32)));
                        std::thread::sleep(delay);
                    }
                }
            }
        }

        Err(last_error.unwrap())
    }
}

#[cfg(not(target_os = "wasi"))]
pub use native::{get, get_with_retry};

#[cfg(target_os = "wasi")]
pub use crate::wasi::http::{get, get_with_retry};
