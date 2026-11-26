//! Optional station-board rows from TrainData `getTrainSchedule19Rec`.
//!
//! The trip-update headsign is the track source that does not add RailData
//! calls. This parser is the fallback used only when that feature is turned on.

use chrono::NaiveDate;

use super::catalog::Station;

/// One departure whose track can fill a stop that had no `Track:` headsign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardRow {
    /// Train number from `TRAIN_ID`.
    pub train_id: String,
    /// `CONNECTING_TRAIN_ID` when it is non-empty.
    pub connecting: Option<String>,
    /// Public track label after the station translation.
    pub track: String,
    /// Eastern civil date of `SCHED_DEP_DATE`.
    pub departure_date: NaiveDate,
}

/// Parses a station-board JSON object into public-track rows.
///
/// Rows whose track is not in `station`'s catalog are dropped. The date is the
/// civil date printed on `SCHED_DEP_DATE`, which is station local time.
///
/// # Arguments
///
/// * `bytes` — JSON object with an `ITEMS` array. A synthetic fixture is enough.
/// * `station` — Station the board was requested for.
///
/// # Returns
///
/// The rows that named a known track. An unreadable body yields an empty list.
///
/// # Errors
///
/// This function does not return an error. Malformed JSON is an empty board.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::{parse_board, station};
///
/// let raw = br#"{
///   "ITEMS": [{
///     "TRAIN_ID": "3846",
///     "CONNECTING_TRAIN_ID": "",
///     "TRACK": "4",
///     "SCHED_DEP_DATE": "15-Jan-2026 10:05:00 AM"
///   }]
/// }"#;
/// let rows = parse_board(raw, station("TS").expect("lower"));
/// assert_eq!(rows[0].track, "E");
/// assert_eq!(rows[0].train_id, "3846");
/// ```
pub fn parse_board(bytes: &[u8], station: &Station) -> Vec<BoardRow> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Vec::new();
    };
    let Some(items) = value.get("ITEMS").and_then(|items| items.as_array()) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for item in items {
        let Some(train_id) = text(item, "TRAIN_ID") else {
            continue;
        };
        let Some(track_raw) = text(item, "TRACK") else {
            continue;
        };
        let Some(track) = station.public_track(track_raw) else {
            continue;
        };
        let Some(when) = text(item, "SCHED_DEP_DATE").and_then(parse_board_date) else {
            continue;
        };
        let connecting = text(item, "CONNECTING_TRAIN_ID")
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        rows.push(BoardRow {
            train_id: train_id.to_string(),
            connecting,
            track: track.to_string(),
            departure_date: when,
        });
    }
    rows
}

/// Reads a string field.
///
/// # Arguments
///
/// * `item` — One `ITEMS` element.
/// * `key` — Field name.
///
/// # Returns
///
/// The trimmed text, or `None` when the field is missing or not a string.
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
/// use njt_gtfs_proxy::tracks::parse_board;
/// use njt_gtfs_proxy::tracks::station;
///
/// assert!(parse_board(b"{}", station("NY").expect("ny")).is_empty());
/// ```
fn text<'a>(item: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    item.get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Parses `30-May-2024 11:52:00 AM` into a civil date.
///
/// # Arguments
///
/// * `text` — `SCHED_DEP_DATE` value.
///
/// # Returns
///
/// The date, or `None` when the text is not that shape.
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
/// use njt_gtfs_proxy::tracks::{parse_board, station};
///
/// let raw = br#"{"ITEMS":[{"TRAIN_ID":"1","TRACK":"1","SCHED_DEP_DATE":"not-a-date"}]}"#;
/// assert!(parse_board(raw, station("NY").expect("ny")).is_empty());
/// ```
fn parse_board_date(text: &str) -> Option<NaiveDate> {
    for format in ["%d-%b-%Y %I:%M:%S %p", "%d-%b-%Y %I:%M %p"] {
        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(text.trim(), format) {
            return Some(parsed.date());
        }
    }
    None
}
