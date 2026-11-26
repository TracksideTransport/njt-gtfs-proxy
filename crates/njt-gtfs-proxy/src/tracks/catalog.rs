//! Stations where this proxy publishes a platform stop for each track.
//!
//! The list is fixed so `/rail/gtfs.zip` does not gain or lose stops between
//! polls. Labels are the public track a rider sees. A few stations report a
//! railroad number in realtime; [`Station::public_track`] maps those to the
//! public label.

/// One rail station that receives platform stops.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::station;
///
/// let ny = station("NY").expect("New York");
/// assert_eq!(ny.stop_id, "105");
/// assert_eq!(ny.public_track("11"), Some("11"));
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Station {
    /// RailData two-character code. The developer GTFS uses this as `stop_code`.
    pub code: &'static str,
    /// `stop_id` observed in the developer rail GTFS dated 2026-11-19.
    pub stop_id: &'static str,
    /// Uppercase fragment that must appear in `stop_name`.
    pub name_fragment: &'static str,
    /// Public track labels, in display order.
    pub tracks: &'static [&'static str],
    /// Railroad value to public label. Compared without case.
    pub translations: &'static [(&'static str, &'static str)],
}

impl Station {
    /// Maps a realtime track token to a public label from [`Station::tracks`].
    ///
    /// # Arguments
    ///
    /// * `raw` — Text after `Track:` or a station-board `TRACK` value.
    ///
    /// # Returns
    ///
    /// The catalog label, or `None` when `raw` is empty or not one of this
    /// station's tracks.
    ///
    /// # Errors
    ///
    /// This method does not return an error.
    ///
    /// # Panics
    ///
    /// This method does not panic.
    ///
    /// # Examples
    ///
    /// ```
    /// use njt_gtfs_proxy::tracks::station;
    ///
    /// let summit = station("ST").expect("Summit");
    /// assert_eq!(summit.public_track("Single"), Some("S"));
    /// assert_eq!(summit.public_track("Track:1"), None);
    /// ```
    pub fn public_track(self, raw: &str) -> Option<&'static str> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let translated = self
            .translations
            .iter()
            .find(|(from, _)| from.eq_ignore_ascii_case(raw))
            .map(|(_, to)| *to)
            .unwrap_or(raw);
        self.tracks
            .iter()
            .copied()
            .find(|track| track.eq_ignore_ascii_case(translated))
    }
}

/// Public tracks at New York Penn Station.
const NY_TRACKS: &[&str] = &[
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17",
    "18", "19", "20", "21",
];
/// Public tracks at Hoboken Terminal.
const HB_TRACKS: &[&str] = &[
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16", "17",
];
/// Public tracks at Trenton Transit Center.
const TR_TRACKS: &[&str] = &["1", "2", "3", "4", "5"];
/// Public tracks at Newark Broad Street.
const ND_TRACKS: &[&str] = &["1", "2", "3"];
/// Public tracks at Newark Penn Station.
const NP_TRACKS: &[&str] = &["A", "1", "2", "3", "4", "5", "H"];
/// Public tracks at Secaucus upper level.
const SE_TRACKS: &[&str] = &["2", "3", "A", "B"];
/// Public tracks at Secaucus lower level.
const TS_TRACKS: &[&str] = &["E", "F", "G", "H"];
/// Public tracks at Newark Airport.
const NA_TRACKS: &[&str] = &["A", "B"];
/// Passenger tracks at Metropark. Inner tracks are not platforms.
const MP_TRACKS: &[&str] = &["1", "4"];
/// Public tracks at Summit, including the single-track label `S`.
const ST_TRACKS: &[&str] = &["1", "2", "S"];

/// Railroad track numbers that Secaucus lower shows as letters.
const TS_TRANSLATIONS: &[(&str, &str)] = &[("1", "G"), ("2", "F"), ("3", "H"), ("4", "E")];
/// Railroad track `0` at Newark Airport is public track A.
const NA_TRANSLATIONS: &[(&str, &str)] = &[("0", "A")];
/// Railroad track `2` at Metropark is public track 1.
const MP_TRANSLATIONS: &[(&str, &str)] = &[("2", "1")];
/// Summit's `Single` label is public track S.
const ST_TRANSLATIONS: &[(&str, &str)] = &[("Single", "S")];

/// Stations polled and published by default, in poll order.
///
/// Track lists come from station directories and platform surveys: New York
/// Penn 1–21, Newark Penn A/1–5/H, Secaucus upper 2/3/A/B, Secaucus lower
/// E–H, Hoboken 1–17, Trenton 1–5, Newark Airport A/B, Metropark passenger
/// tracks 1 and 4, Newark Broad Street 1–3, and Summit 1/2/S.
pub static STATIONS: &[Station] = &[
    Station {
        code: "NY",
        stop_id: "105",
        name_fragment: "NEW YORK",
        tracks: NY_TRACKS,
        translations: &[],
    },
    Station {
        code: "NP",
        stop_id: "107",
        name_fragment: "NEWARK PENN",
        tracks: NP_TRACKS,
        translations: &[],
    },
    Station {
        code: "SE",
        stop_id: "38187",
        name_fragment: "SECAUCUS UPPER",
        tracks: SE_TRACKS,
        translations: &[],
    },
    Station {
        code: "TS",
        stop_id: "38174",
        name_fragment: "SECAUCUS LOWER",
        tracks: TS_TRACKS,
        translations: TS_TRANSLATIONS,
    },
    Station {
        code: "HB",
        stop_id: "63",
        name_fragment: "HOBOKEN",
        tracks: HB_TRACKS,
        translations: &[],
    },
    Station {
        code: "TR",
        stop_id: "148",
        name_fragment: "TRENTON",
        tracks: TR_TRACKS,
        translations: &[],
    },
    Station {
        code: "NA",
        stop_id: "37953",
        name_fragment: "NEWARK AIRPORT",
        tracks: NA_TRACKS,
        translations: NA_TRANSLATIONS,
    },
    Station {
        code: "MP",
        stop_id: "83",
        name_fragment: "METROPARK",
        tracks: MP_TRACKS,
        translations: MP_TRANSLATIONS,
    },
    Station {
        code: "ND",
        stop_id: "106",
        name_fragment: "NEWARK BROAD",
        tracks: ND_TRACKS,
        translations: &[],
    },
    Station {
        code: "ST",
        stop_id: "145",
        name_fragment: "SUMMIT",
        tracks: ST_TRACKS,
        translations: ST_TRANSLATIONS,
    },
];

/// Looks up a station by its two-character code.
///
/// # Arguments
///
/// * `code` — Station code such as `NY`. Case is ignored.
///
/// # Returns
///
/// The catalog entry, or `None` when `code` is not in [`STATIONS`].
///
/// # Errors
///
/// This function does not return an error.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::station;
///
/// assert!(station("ny").is_some());
/// assert!(station("ZZ").is_none());
/// ```
pub fn station(code: &str) -> Option<&'static Station> {
    let code = code.trim();
    STATIONS
        .iter()
        .find(|station| station.code.eq_ignore_ascii_case(code))
}

/// Default station codes, in poll order.
///
/// # Arguments
///
/// This function takes no arguments.
///
/// # Returns
///
/// The `code` of every entry in [`STATIONS`].
///
/// # Errors
///
/// This function does not return an error.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::default_codes;
///
/// assert_eq!(default_codes()[0], "NY");
/// assert_eq!(default_codes().len(), 10);
/// ```
pub fn default_codes() -> Vec<&'static str> {
    STATIONS.iter().map(|station| station.code).collect()
}

/// Resolves `codes` against [`STATIONS`], preserving order and dropping duplicates.
///
/// # Arguments
///
/// * `codes` — Comma-separated pieces already split by the caller.
///
/// # Returns
///
/// The matching stations.
///
/// # Errors
///
/// Returns the unknown code when a piece is not in the catalog.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::resolve_codes;
///
/// let stations = resolve_codes(["ny", "NP"]).expect("codes");
/// assert_eq!(stations.len(), 2);
/// assert!(resolve_codes(["nope"]).is_err());
/// ```
pub fn resolve_codes<I, S>(codes: I) -> Result<Vec<&'static Station>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = Vec::new();
    for code in codes {
        let code = code.as_ref().trim();
        if code.is_empty() {
            continue;
        }
        let Some(station) = station(code) else {
            return Err(code.to_string());
        };
        if out
            .iter()
            .any(|existing: &&Station| existing.code == station.code)
        {
            continue;
        }
        out.push(station);
    }
    Ok(out)
}

/// Parent stop id added above `stop_id`.
///
/// # Arguments
///
/// * `stop_id` — The existing station stop.
///
/// # Returns
///
/// `{stop_id}:station`, the same shape used by the Amtrak GTFS-RT proxy.
///
/// # Errors
///
/// This function does not return an error.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::parent_stop_id;
///
/// assert_eq!(parent_stop_id("105"), "105:station");
/// ```
pub fn parent_stop_id(stop_id: &str) -> String {
    format!("{stop_id}:station")
}

/// Platform stop id for `track` at `stop_id`.
///
/// # Arguments
///
/// * `stop_id` — The existing station stop.
/// * `track` — Public track label.
///
/// # Returns
///
/// `{stop_id}:track:{track}`.
///
/// # Errors
///
/// This function does not return an error.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::platform_stop_id;
///
/// assert_eq!(platform_stop_id("105", "11"), "105:track:11");
/// ```
pub fn platform_stop_id(stop_id: &str, track: &str) -> String {
    format!("{stop_id}:track:{track}")
}

/// How many station-board calls a full day of polling would make.
///
/// # Arguments
///
/// * `stations` — Number of stations in the poll list.
/// * `poll_secs` — Seconds between the start of one full cycle and the next.
///
/// # Returns
///
/// `stations * (86400 / poll_secs)`, or 0 when polling is disabled.
///
/// # Errors
///
/// This function does not return an error.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::board_calls_per_day;
///
/// assert_eq!(board_calls_per_day(10, 600), 1440);
/// assert_eq!(board_calls_per_day(10, 0), 0);
/// ```
pub fn board_calls_per_day(stations: usize, poll_secs: u64) -> u64 {
    if poll_secs == 0 || stations == 0 {
        return 0;
    }
    stations as u64 * (86_400 / poll_secs)
}
