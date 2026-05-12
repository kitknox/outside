pub mod api;
pub mod context;
pub mod output;
pub mod settings;
pub mod tui;
pub mod utils;

#[cfg(target_os = "wasi")]
pub mod wasi;

use crate::api::location::LocationData;
use crate::api::weather;
use crate::settings::{OutputFormat, Settings, Units};
use anyhow::Result;

/// Main entry point for the outside weather CLI application.
///
/// The flow is the same on both targets:
/// 1. Build configuration from config file and CLI arguments
/// 2. Resolve location data (with caching)
/// 3. Fetch weather data (with caching)
/// 4. Build context for template rendering
/// 5. Render and output
///
/// Native: tokio runtime, signal-based Ctrl-C, threads for TUI fetches.
/// WASI: sync top-to-bottom, Ctrl-C handled host-side via cooked-mode kill,
/// TUI fetches block the UI briefly (no threads).
fn main() -> Result<()> {
    let config_file = dirs_next::config_dir()
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from))
        .or_else(|| dirs_next::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(env!("CARGO_PKG_NAME"))
        .join("config.yaml");

    let s = Settings::build(vec![config_file], std::env::args_os())?;

    // TUI mode is incompatible with streaming mode
    if s.stream && matches!(s.output, OutputFormat::Tui) {
        eprintln!("Error: TUI mode cannot be used with streaming mode.");
        std::process::exit(1);
    }

    #[cfg(target_os = "wasi")]
    {
        run_wasi(s)
    }

    #[cfg(not(target_os = "wasi"))]
    {
        run_native(s)
    }
}

/// Native entry: bootstraps tokio explicitly (no #[tokio::main] so that the
/// WASI build can stay sync) and dispatches to the streaming or single
/// run-loop.
#[cfg(not(target_os = "wasi"))]
fn run_native(s: Settings) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async move {
        if s.stream {
            run_streaming_mode(s).await
        } else {
            run_single_mode(s).await
        }
    })
}

/// WASI entry: sync top-to-bottom. The streaming loop uses `thread::sleep`
/// rather than tokio timers; Ctrl-C in cooked mode kills the WASM process
/// from the host side, so there's no signal handling inside this loop.
#[cfg(target_os = "wasi")]
fn run_wasi(s: Settings) -> Result<()> {
    if s.stream {
        // Emit immediately so users see something before the first interval.
        if let Err(e) = output_weather_data_sync(&s) {
            eprintln!("Error fetching initial weather data: {e}");
        }
        loop {
            std::thread::sleep(std::time::Duration::from_secs(s.interval));
            if let Err(e) = output_weather_data_sync(&s) {
                eprintln!("Error fetching weather data: {e}");
            }
        }
    }
    output_weather_data_sync(&s)
}

/// Streaming mode (native only). Ticks every `settings.interval` seconds
/// until Ctrl-C arrives via `tokio::signal`.
#[cfg(not(target_os = "wasi"))]
async fn run_streaming_mode(settings: Settings) -> Result<()> {
    use std::time::Duration;
    use tokio::signal;
    use tokio::time::interval;

    let mut timer = interval(Duration::from_secs(settings.interval));

    if let Err(e) = output_weather_data(&settings).await {
        eprintln!("Error fetching initial weather data: {e}");
    }

    // Skip the first tick since interval.tick() fires immediately
    timer.tick().await;

    loop {
        tokio::select! {
            _ = timer.tick() => {
                if let Err(e) = output_weather_data(&settings).await {
                    eprintln!("Error fetching weather data: {e}");
                    continue;
                }
            }
            _ = signal::ctrl_c() => {
                if cfg!(debug_assertions) {
                    eprintln!("Received interrupt signal, shutting down gracefully");
                }
                break;
            }
        }
    }

    Ok(())
}

#[cfg(not(target_os = "wasi"))]
async fn run_single_mode(settings: Settings) -> Result<()> {
    output_weather_data(&settings).await
}

#[cfg(not(target_os = "wasi"))]
async fn output_weather_data(settings: &Settings) -> Result<()> {
    output_weather_data_sync(settings)
}

/// The core weather data pipeline. Sync — the underlying location and
/// weather fetches are blocking on both targets (isahc and our wasi http
/// client both return synchronously), so the native `async fn` wrappers
/// above are effectively cosmetic.
fn output_weather_data_sync(settings: &Settings) -> Result<()> {
    let loc = LocationData::get_cached(settings.clone())?;
    let weather =
        weather::Weather::get_cached(loc.latitude, loc.longitude, settings.clone())?;

    let context = context::Context::build(weather, loc, settings.clone());
    let output = settings.output.render_fn()(context, settings.clone());

    println!("{output}");
    Ok(())
}
