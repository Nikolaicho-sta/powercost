//! Region detection: a best guess from the system locale and timezone. It never touches the
//! network (the optional IP lookup is a separate, opt-in step done by the UI). The first-run
//! wizard always lets the user confirm or override the guess.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuessSource {
    Timezone,
    Locale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// ISO 3166-1 alpha-2, upper case.
    pub country: String,
    pub source: GuessSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionGuess {
    /// Best first; empty when nothing could be inferred.
    pub candidates: Vec<Candidate>,
}

impl RegionGuess {
    pub fn best(&self) -> Option<&str> {
        self.candidates.first().map(|c| c.country.as_str())
    }
}

/// IANA timezone -> country for the zones people actually use; not exhaustive (the wizard
/// falls back to the locale and then to a manual choice).
const TZ_COUNTRY: &[(&str, &str)] = &[
    ("Europe/Sofia", "BG"), ("Europe/Berlin", "DE"), ("Europe/Vienna", "AT"), ("Europe/Paris", "FR"),
    ("Europe/Madrid", "ES"), ("Europe/Lisbon", "PT"), ("Europe/Rome", "IT"), ("Europe/Athens", "GR"),
    ("Europe/Bucharest", "RO"), ("Europe/Belgrade", "RS"), ("Europe/Zagreb", "HR"), ("Europe/Ljubljana", "SI"),
    ("Europe/Budapest", "HU"), ("Europe/Prague", "CZ"), ("Europe/Bratislava", "SK"), ("Europe/Warsaw", "PL"),
    ("Europe/Vilnius", "LT"), ("Europe/Riga", "LV"), ("Europe/Tallinn", "EE"), ("Europe/Helsinki", "FI"),
    ("Europe/Stockholm", "SE"), ("Europe/Oslo", "NO"), ("Europe/Copenhagen", "DK"), ("Europe/Amsterdam", "NL"),
    ("Europe/Brussels", "BE"), ("Europe/Luxembourg", "LU"), ("Europe/Zurich", "CH"), ("Europe/Dublin", "IE"),
    ("Europe/London", "GB"), ("Europe/Istanbul", "TR"), ("Europe/Kyiv", "UA"), ("Europe/Kiev", "UA"),
    ("Europe/Skopje", "MK"), ("Europe/Tirane", "AL"), ("Europe/Sarajevo", "BA"), ("Europe/Chisinau", "MD"),
    ("America/New_York", "US"), ("America/Chicago", "US"), ("America/Denver", "US"), ("America/Los_Angeles", "US"),
    ("America/Toronto", "CA"), ("America/Vancouver", "CA"), ("America/Mexico_City", "MX"), ("America/Sao_Paulo", "BR"),
    ("Australia/Sydney", "AU"), ("Australia/Melbourne", "AU"), ("Pacific/Auckland", "NZ"),
    ("Asia/Tokyo", "JP"), ("Asia/Seoul", "KR"), ("Asia/Kolkata", "IN"), ("Asia/Singapore", "SG"),
];

fn country_from_locale(locale: &str) -> Option<String> {
    // "bg-BG", "en_US", "en-GB-u-ca-gregory", "sr-Latn-RS": first 2-letter ALL-CAPS subtag after the language.
    locale
        .split(['-', '_'])
        .skip(1)
        .find(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic()))
        .map(|p| p.to_ascii_uppercase())
}

/// Timezone is physical (where the PC is), the locale is a language preference, so the timezone
/// wins; both are returned so the wizard can show the alternative.
pub fn guess_region(locale: &str, timezone: &str) -> RegionGuess {
    let mut candidates = Vec::new();
    if let Some((_, c)) = TZ_COUNTRY.iter().find(|(tz, _)| tz.eq_ignore_ascii_case(timezone.trim())) {
        candidates.push(Candidate { country: (*c).to_string(), source: GuessSource::Timezone });
    }
    if let Some(c) = country_from_locale(locale) {
        if !candidates.iter().any(|x| x.country == c) {
            candidates.push(Candidate { country: c, source: GuessSource::Locale });
        }
    }
    RegionGuess { candidates }
}