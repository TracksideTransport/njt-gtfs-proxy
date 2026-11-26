//! Track labels carried on `StopTimeUpdate.stop_time_properties.stop_headsign`.
//!
//! A test-feed snapshot on 26 Nov 2026 showed values such as `Track:1`,
//! `Track:11`, and `Track:Single` on the trip updates NJ Transit already returns.
//! Parsing that text does not call RailData again.

/// Returns the track token from a `Track:` headsign.
///
/// # Arguments
///
/// * `headsign` — `stop_headsign` text. A leading `Track:` or `track:` is required.
///
/// # Returns
///
/// The trimmed token after the prefix, or `None` when the headsign is not a
/// track assignment.
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
/// use njt_gtfs_proxy::tracks::track_from_headsign;
///
/// assert_eq!(track_from_headsign("Track:11"), Some("11"));
/// assert_eq!(track_from_headsign("Track:Single"), Some("Single"));
/// assert!(track_from_headsign("Delayed").is_none());
/// ```
pub fn track_from_headsign(headsign: &str) -> Option<&str> {
    let rest = headsign
        .strip_prefix("Track:")
        .or_else(|| headsign.strip_prefix("track:"))?;
    let rest = rest.trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
}
