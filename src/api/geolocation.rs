use crate::api::client;
use crate::api::location::*;
use crate::utils;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GeoLocation {
    pub results: Vec<Results>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Results {
    pub name: String,
    pub country_code: String,
    pub latitude: f64,
    pub longitude: f64,
    /// First-level administrative division (state for US, province for CA, etc.)
    /// Open-Meteo returns this as the full name (e.g. "Massachusetts", "Texas").
    /// Optional because it's missing for some city-states and some smaller
    /// entries in the geocoding database.
    #[serde(default)]
    pub admin1: Option<String>,
}

impl Location for GeoLocation {
    /// Fetches location data using the Open-Meteo geocoding API.
    /// Two-part input ("City, CountryCode") — picks the first result, which
    /// is roughly the most populous in that country.
    fn fetch(n: &str, c: &str) -> Result<LocationData> {
        Self::fetch_filtered(n, "", c)
    }
}

impl GeoLocation {
    /// Fetches location data with optional state/admin1 filtering. Empty
    /// `state` falls back to "first result wins". A non-empty state filters
    /// the results list to entries whose `admin1` matches case-insensitively
    /// — handles both full names ("Massachusetts") and US two-letter
    /// abbreviations ("MA") via [`expand_us_state`].
    pub fn fetch_filtered(n: &str, state: &str, c: &str) -> Result<LocationData> {
        let base_url = "https://geocoding-api.open-meteo.com/v1/search";
        let params = vec![
            ("name", n),
            ("countryCode", c),
            ("count", "10"),
            ("language", "en"),
            ("format", "json"),
        ];
        let api_url = utils::urls::builder(base_url, params);

        let body = client::get_with_retry(&api_url, 2)
            .with_context(|| format!("Unable to fetch location data for {n}, {c}"))?;

        let loc: GeoLocation =
            serde_json::from_str(&body).with_context(|| "Failed to parse location response JSON")?;

        if loc.results.is_empty() {
            anyhow::bail!("No location results found for {}, {}", n, c);
        }

        let result = if state.is_empty() {
            // No state given: first result wins (current behavior).
            &loc.results[0]
        } else {
            // Match by admin1. Accept full name OR US 2-letter abbreviation.
            let target = expand_us_state(state).unwrap_or_else(|| state.to_string());
            let chosen = loc.results.iter().find(|r| {
                r.admin1
                    .as_deref()
                    .map(|a| a.eq_ignore_ascii_case(&target) || a.eq_ignore_ascii_case(state))
                    .unwrap_or(false)
            });
            match chosen {
                Some(r) => r,
                None => {
                    // List what we got so the user can correct their input.
                    let alternatives: Vec<String> = loc
                        .results
                        .iter()
                        .filter_map(|r| r.admin1.clone())
                        .collect();
                    let alts = if alternatives.is_empty() {
                        "no admin1 data".to_string()
                    } else {
                        alternatives.join(", ")
                    };
                    anyhow::bail!(
                        "No {n} found in state/region '{state}' (country {c}). \
                         Open-Meteo returned: {alts}"
                    );
                }
            }
        };

        let mut location_data = LocationData {
            city: result.name.to_owned(),
            country_code: result.country_code.to_owned(),
            state: result.admin1.clone().unwrap_or_default(),
            latitude: result.latitude,
            longitude: result.longitude,
            location: String::new(), // filled in by normalize()
            created_at: utils::get_now(),
        };

        location_data.normalize();

        Ok(location_data)
    }
}

/// Expand a US state two-letter abbreviation to its full name. Returns None
/// for inputs that aren't a known US state abbreviation (including full
/// state names — they pass through unchanged at the caller).
pub fn expand_us_state(abbr: &str) -> Option<String> {
    let upper = abbr.trim().to_uppercase();
    let full = match upper.as_str() {
        "AL" => "Alabama",
        "AK" => "Alaska",
        "AZ" => "Arizona",
        "AR" => "Arkansas",
        "CA" => "California",
        "CO" => "Colorado",
        "CT" => "Connecticut",
        "DE" => "Delaware",
        "DC" => "District of Columbia",
        "FL" => "Florida",
        "GA" => "Georgia",
        "HI" => "Hawaii",
        "ID" => "Idaho",
        "IL" => "Illinois",
        "IN" => "Indiana",
        "IA" => "Iowa",
        "KS" => "Kansas",
        "KY" => "Kentucky",
        "LA" => "Louisiana",
        "ME" => "Maine",
        "MD" => "Maryland",
        "MA" => "Massachusetts",
        "MI" => "Michigan",
        "MN" => "Minnesota",
        "MS" => "Mississippi",
        "MO" => "Missouri",
        "MT" => "Montana",
        "NE" => "Nebraska",
        "NV" => "Nevada",
        "NH" => "New Hampshire",
        "NJ" => "New Jersey",
        "NM" => "New Mexico",
        "NY" => "New York",
        "NC" => "North Carolina",
        "ND" => "North Dakota",
        "OH" => "Ohio",
        "OK" => "Oklahoma",
        "OR" => "Oregon",
        "PA" => "Pennsylvania",
        "RI" => "Rhode Island",
        "SC" => "South Carolina",
        "SD" => "South Dakota",
        "TN" => "Tennessee",
        "TX" => "Texas",
        "UT" => "Utah",
        "VT" => "Vermont",
        "VA" => "Virginia",
        "WA" => "Washington",
        "WV" => "West Virginia",
        "WI" => "Wisconsin",
        "WY" => "Wyoming",
        _ => return None,
    };
    Some(full.to_string())
}
