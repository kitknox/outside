use crate::api::location::LocationData;
use crate::api::weather::Weather;
use crate::context::Context;
use crate::tui::constants::*;
use crate::tui::state_manager::TuiStateManager;
use crate::tui::ui_components::UiComponents;
use crate::tui::weather_display::WeatherDisplay;
use cursive::views::{ProgressBar, TextView};
use cursive::Cursive;
#[cfg(not(target_os = "wasi"))]
use std::thread;

pub struct WeatherFetcher {
    state_manager: TuiStateManager,
}

impl WeatherFetcher {
    pub fn new(state_manager: TuiStateManager) -> Self {
        Self { state_manager }
    }

    /// Fetch weather for `location` and route the result through the supplied
    /// callbacks. Native spawns a thread and dispatches via `cb_sink`. WASI
    /// has neither — it runs the fetch synchronously on the UI thread,
    /// repainting the "loading…" state first so users see something during
    /// the brief freeze.
    pub fn fetch_and_update<F, E>(
        &self,
        location: String,
        siv: &mut Cursive,
        success_callback: F,
        error_callback: E,
    ) where
        F: Fn(&mut Cursive, &TuiStateManager, Context) + Send + 'static,
        E: Fn(&mut Cursive, &TuiStateManager, String) + Send + 'static,
    {
        self.state_manager.set_loading(true);
        self.update_ui_loading(siv);

        #[cfg(target_os = "wasi")]
        {
            // We can't force a paint of "Loading…" before the blocking fetch
            // — cursive's redraws are tied to its run loop. So the UI shows
            // stale state for the ~0.5–1s the fetch takes. Acceptable
            // tradeoff for not having background threads under WASI.
            let state_manager_clone = self.state_manager.clone();
            match Self::fetch_weather_for_location(&location, &state_manager_clone) {
                Ok(result) => {
                    state_manager_clone.update_context(result.clone());
                    success_callback(siv, &state_manager_clone, result);
                }
                Err(_) => {
                    state_manager_clone.set_loading(false);
                    let error_message = format!("Failed to fetch location data for: {location}");
                    error_callback(siv, &state_manager_clone, error_message);
                }
            }
        }

        #[cfg(not(target_os = "wasi"))]
        {
            let cb_sink = siv.cb_sink().clone();
            let location_clone = location.clone();
            let state_manager_clone = self.state_manager.clone();

            thread::spawn(move || {
                if let Ok(result) =
                    Self::fetch_weather_for_location(&location_clone, &state_manager_clone)
                {
                    cb_sink
                        .send(Box::new(move |s| {
                            state_manager_clone.update_context(result.clone());
                            success_callback(s, &state_manager_clone, result);
                        }))
                        .unwrap();
                } else {
                    let error_message = format!("Failed to fetch location data for: {location_clone}");
                    cb_sink
                        .send(Box::new(move |s| {
                            state_manager_clone.set_loading(false);
                            error_callback(s, &state_manager_clone, error_message);
                        }))
                        .unwrap();
                }
            });
        }
    }

    pub fn switch_location(&self, siv: &mut Cursive, location: String) {
        let location_clone = location.clone();
        self.fetch_and_update(
            location,
            siv,
            move |s, state_manager, context| {
                state_manager.update_context_with_location(context, location_clone.clone());
                UiComponents::update_weather_display_components(s, state_manager);
            },
            |s, _state_manager, error_message| {
                Self::show_error_dialog(s, &error_message);
            },
        );
    }

    pub fn toggle_units(&self, siv: &mut Cursive) {
        let current_location = self.state_manager.get_current_location();
        self.state_manager.toggle_units();
        self.state_manager.set_loading(true);

        siv.call_on_name(WEATHER_HEADER_NAME, |view: &mut TextView| {
            view.set_content(WeatherDisplay::format_units_switching_message());
        });
        siv.call_on_name(WEATHER_CURRENT_NAME, |view: &mut TextView| {
            view.set_content(WeatherDisplay::format_wait_message());
        });
        siv.call_on_name(WEATHER_FORECAST_NAME, |view: &mut TextView| {
            view.set_content("");
        });

        #[cfg(target_os = "wasi")]
        {
            // Same caveat as fetch_and_update: no forced repaint before the
            // blocking fetch under WASI. Users see the prior weather data
            // briefly, then it pops to the new units after the fetch.
            let state_manager_clone = self.state_manager.clone();
            match Self::fetch_weather_for_location(&current_location, &state_manager_clone) {
                Ok(result) => {
                    state_manager_clone
                        .update_context_with_location(result, current_location.clone());
                    UiComponents::update_weather_display_components(siv, &state_manager_clone);
                }
                Err(_) => {
                    state_manager_clone.set_loading(false);
                    Self::show_error_dialog(siv, "Failed to fetch weather data with new units");
                    UiComponents::update_weather_display_components(siv, &state_manager_clone);
                }
            }
        }

        #[cfg(not(target_os = "wasi"))]
        {
            let cb_sink = siv.cb_sink().clone();
            let state_manager_clone = self.state_manager.clone();

            thread::spawn(move || {
                if let Ok(result) =
                    Self::fetch_weather_for_location(&current_location, &state_manager_clone)
                {
                    let location_for_update = current_location.clone();
                    cb_sink
                        .send(Box::new(move |s| {
                            state_manager_clone
                                .update_context_with_location(result, location_for_update);
                            UiComponents::update_weather_display_components(s, &state_manager_clone);
                        }))
                        .unwrap();
                } else {
                    cb_sink
                        .send(Box::new(move |s| {
                            state_manager_clone.set_loading(false);
                            Self::show_error_dialog(s, "Failed to fetch weather data with new units");
                            UiComponents::update_weather_display_components(s, &state_manager_clone);
                        }))
                        .unwrap();
                }
            });
        }
    }

    /// Background auto-refresh. Native spawns a polling thread; WASI has no
    /// threads, so this becomes a no-op and the cache simply ages until the
    /// user triggers a fetch via the location-switch path.
    #[cfg(not(target_os = "wasi"))]
    pub fn setup_auto_refresh(&self, siv: &mut Cursive) {
        let cb_sink = siv.cb_sink().clone();
        let state_manager_clone = self.state_manager.clone();

        thread::spawn(move || loop {
            thread::sleep(std::time::Duration::from_secs(AUTO_REFRESH_INTERVAL));

            if state_manager_clone.needs_refresh() {
                let current_location = state_manager_clone.get_current_location();
                let state_for_refresh = state_manager_clone.clone();

                let _ = cb_sink.send(Box::new(move |s| {
                    let fetcher = WeatherFetcher::new(state_for_refresh);
                    fetcher.switch_location(s, current_location);
                }));
            } else {
                let state_for_display = state_manager_clone.clone();
                let _ = cb_sink.send(Box::new(move |s| {
                    state_for_display.update_cache_age();
                    UiComponents::update_weather_display_components(s, &state_for_display);
                }));
            }
        });
    }

    #[cfg(target_os = "wasi")]
    pub fn setup_auto_refresh(&self, _siv: &mut Cursive) {
        // No threads under wasm32-wasip1, so auto-refresh is disabled. The
        // cache_age progress bar still updates whenever the UI repaints, and
        // any user action (Enter on the bookmarks list) triggers a fresh
        // fetch through fetch_and_update.
    }

    fn update_ui_loading(&self, siv: &mut Cursive) {
        siv.call_on_name(WEATHER_HEADER_NAME, |view: &mut TextView| {
            view.set_content(WeatherDisplay::format_loading_message());
        });
        siv.call_on_name(WEATHER_CURRENT_NAME, |view: &mut TextView| {
            view.set_content(WeatherDisplay::format_wait_message());
        });
        siv.call_on_name(WEATHER_FORECAST_NAME, |view: &mut TextView| {
            view.set_content("");
        });
        siv.call_on_name(DATA_AGE_PROGRESS_NAME, |view: &mut ProgressBar| {
            view.set_value(0);
        });
    }

    fn show_error_dialog(siv: &mut Cursive, message: &str) {
        siv.add_layer(cursive::views::Dialog::text(message).title("Error").button("OK", |s| {
            s.pop_layer();
        }));
    }

    fn fetch_weather_for_location(
        location: &str,
        state_manager: &TuiStateManager,
    ) -> Result<Context, Box<dyn std::error::Error + Send + Sync>> {
        let mut settings = state_manager.get_settings();

        if location == "Automatic" {
            settings.location = String::new();
        } else {
            let parts: Vec<&str> = location.split(',').collect();
            if parts.len() != 2 {
                return Err("Invalid location format".into());
            }
            settings.location = location.to_string();
        }

        let location_data = LocationData::get_cached(settings.clone())?;
        let weather_data =
            Weather::get_cached(location_data.latitude, location_data.longitude, settings.clone())?;
        let context = Context::build(weather_data, location_data, settings);

        Ok(context)
    }
}

impl Clone for WeatherFetcher {
    fn clone(&self) -> Self {
        Self { state_manager: self.state_manager.clone() }
    }
}

impl Clone for TuiStateManager {
    fn clone(&self) -> Self {
        Self { state: self.state.clone() }
    }
}
