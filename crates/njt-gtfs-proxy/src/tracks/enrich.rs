//! Writes platform assignments and missing trip ids into a trip-update feed.
//!
//! NJ Transit's trip updates already put the track in
//! `stop_time_properties.stop_headsign` (`Track:11`). This module copies that
//! label onto `assigned_stop_id` for the matching platform stop and leaves the
//! headsign in place. Added trips often have an empty `trip_id` whose feed
//! entity id is `trips.trip_short_name`; that id is filled in when the static
//! zip identifies one trip.

use std::collections::HashMap;

use prost::Message;

use super::board::BoardRow;
use super::catalog::{platform_stop_id, Station};
use super::headsign::track_from_headsign;
use super::index::{same_train, StaticIndex};
use gtfs_realtime::trip_update::stop_time_update::StopTimeProperties;
use gtfs_realtime::FeedMessage;

/// Realtime track context for one enrichment.
pub struct EnrichInput<'a> {
    /// Stations whose platform stops are in the zip.
    pub stations: &'a [&'static Station],
    /// Static trip and stop index. Empty when the zip has not been read.
    pub index: &'a StaticIndex,
    /// Fresh station-board rows keyed by station code. Empty when boards are off.
    pub boards: &'a HashMap<String, Vec<BoardRow>>,
}

/// Sets `assigned_stop_id` and fills an empty `trip_id`.
///
/// # Arguments
///
/// * `bytes` — Upstream `TripUpdates` protobuf.
/// * `input` — Catalog, static index, and optional board rows.
///
/// # Returns
///
/// `None` when `bytes` is not a feed or nothing changed. `Some` is the re-encoded
/// feed. Arrival and departure times, `stop_id`, `stop_headsign`, and a `trip_id`
/// that was already set are not modified.
///
/// # Errors
///
/// This function does not return an error. An undecodable body is `None`.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::{enrich_trip_updates, EnrichInput, StaticIndex};
///
/// let input = EnrichInput {
///     stations: &[],
///     index: &StaticIndex::default(),
///     boards: &Default::default(),
/// };
/// assert!(enrich_trip_updates(b"not-a-feed", &input).is_none());
/// ```
pub fn enrich_trip_updates(bytes: &[u8], input: &EnrichInput<'_>) -> Option<Vec<u8>> {
    let mut message = FeedMessage::decode(bytes).ok()?;
    let mut changed = false;
    for entity in &mut message.entity {
        let Some(update) = entity.trip_update.as_mut() else {
            continue;
        };
        let start_date = update.trip.start_date.clone();
        if trip_id_blank(&update.trip.trip_id) {
            if let Some(trip_id) = input
                .index
                .trip_for_train(&entity.id, start_date.as_deref())
            {
                update.trip.trip_id = Some(trip_id.to_string());
                changed = true;
            }
        }
        let trains = train_names(&entity.id, update.trip.trip_id.as_deref(), input.index);
        for stop in &mut update.stop_time_update {
            let Some(stop_id) = stop.stop_id.clone() else {
                continue;
            };
            let Some(station) = station_for_stop(&stop_id, input) else {
                continue;
            };
            if stop
                .stop_time_properties
                .as_ref()
                .and_then(|properties| properties.assigned_stop_id.as_ref())
                .is_some_and(|value| !value.trim().is_empty())
            {
                continue;
            }
            let had_properties = stop.stop_time_properties.is_some();
            let properties = stop
                .stop_time_properties
                .get_or_insert_with(StopTimeProperties::default);
            let from_headsign = properties
                .stop_headsign
                .as_deref()
                .and_then(track_from_headsign)
                .and_then(|token| station.public_track(token));
            let track = from_headsign.or_else(|| {
                board_track(
                    station,
                    &trains,
                    start_date.as_deref(),
                    input.boards.get(station.code),
                )
            });
            let Some(track) = track else {
                if !had_properties {
                    stop.stop_time_properties = None;
                }
                continue;
            };
            properties.assigned_stop_id = Some(platform_stop_id(&stop_id, track));
            changed = true;
        }
    }
    if !changed {
        return None;
    }
    Some(message.encode_to_vec())
}

/// Reports whether `trip_id` was omitted or blank.
///
/// # Arguments
///
/// * `trip_id` — `TripDescriptor.trip_id`.
///
/// # Returns
///
/// `true` when there is nothing to keep.
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
/// use njt_gtfs_proxy::tracks::enrich_trip_updates;
///
/// let _ = enrich_trip_updates;
/// ```
fn trip_id_blank(trip_id: &Option<String>) -> bool {
    trip_id
        .as_ref()
        .map(|id| id.trim().is_empty())
        .unwrap_or(true)
}

/// Train numbers that can match a board row for this entity.
///
/// # Arguments
///
/// * `entity_id` — Feed entity id, which added trips use as the train number.
/// * `trip_id` — Static trip id when one is already known.
/// * `index` — Scheduled trips.
///
/// # Returns
///
/// The entity id, plus the scheduled train number when `trip_id` is in the index.
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
/// use njt_gtfs_proxy::tracks::same_train;
///
/// assert!(same_train("0510", "510"));
/// ```
fn train_names<'a>(
    entity_id: &'a str,
    trip_id: Option<&str>,
    index: &'a StaticIndex,
) -> Vec<&'a str> {
    let mut names = vec![entity_id];
    if let Some(trip_id) = trip_id {
        if let Some(trip) = index.trips.iter().find(|trip| trip.trip_id == trip_id) {
            names.push(trip.train.as_str());
        }
    }
    names
}

/// Resolves a realtime `stop_id` to a catalog station.
///
/// # Arguments
///
/// * `stop_id` — `StopTimeUpdate.stop_id`.
/// * `input` — Index and catalog.
///
/// # Returns
///
/// The station whose feed stop is `stop_id`. The static index wins. The catalog
/// `stop_id` is the fallback before the zip has been read.
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
/// assert_eq!(station("NY").unwrap().stop_id, "105");
/// ```
fn station_for_stop<'a>(stop_id: &str, input: &'a EnrichInput<'_>) -> Option<&'a Station> {
    if let Some(code) = input.index.stop_stations.get(stop_id) {
        return input
            .stations
            .iter()
            .copied()
            .find(|station| station.code == code);
    }
    input
        .stations
        .iter()
        .copied()
        .find(|station| station.stop_id == stop_id)
}

/// Picks a board track for `station` when the headsign did not name one.
///
/// # Arguments
///
/// * `station` — Stop's station. The track must be one of its public labels.
/// * `trains` — Candidate train numbers.
/// * `start_date` — Trip start date, when present.
/// * `rows` — That station's fresh board.
///
/// # Returns
///
/// A public track label. A dated trip only matches a board row on that date or
/// the next civil date. Several rows without a unique date are skipped.
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
/// assert!(parse_board(b"{}", station("NY").unwrap()).is_empty());
/// ```
fn board_track<'a>(
    station: &'a Station,
    trains: &[&str],
    start_date: Option<&str>,
    rows: Option<&'a Vec<BoardRow>>,
) -> Option<&'a str> {
    let rows = rows?;
    let mut hits: Vec<&BoardRow> = rows
        .iter()
        .filter(|row| {
            trains.iter().any(|train| {
                same_train(train, &row.train_id)
                    || row
                        .connecting
                        .as_deref()
                        .is_some_and(|connecting| same_train(train, connecting))
            })
        })
        .collect();
    if let Some(date) = start_date.map(str::trim).filter(|date| !date.is_empty()) {
        let exact: Vec<&BoardRow> = hits
            .iter()
            .copied()
            .filter(|row| row.departure_date.format("%Y%m%d").to_string() == date)
            .collect();
        if !exact.is_empty() {
            hits = exact;
        } else {
            hits.retain(|row| {
                row.departure_date
                    .pred_opt()
                    .is_some_and(|previous| previous.format("%Y%m%d").to_string() == date)
            });
        }
    } else if hits.len() > 1 {
        return None;
    }
    let row = hits.first()?;
    station.public_track(&row.track)
}
