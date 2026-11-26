//! Track assignments for the rail feeds.
//!
//! Trip updates from NJ Transit carry the track in `stop_headsign` (`Track:11`).
//! The proxy turns that into `assigned_stop_id` on a platform stop it adds to
//! the static zip, in the MBTA commuter-rail shape. Station-board polling is a
//! separate fallback and is off unless configured.
//!
//! # Examples
//!
//! ```
//! use njt_gtfs_proxy::tracks::{board_calls_per_day, track_from_headsign};
//!
//! assert_eq!(track_from_headsign("Track:1"), Some("1"));
//! assert_eq!(board_calls_per_day(10, 600), 1440);
//! ```

mod board;
mod catalog;
mod enrich;
mod headsign;
mod hub;
mod index;
mod platforms;

pub use board::{parse_board, BoardRow};
pub use catalog::{
    board_calls_per_day, default_codes, parent_stop_id, platform_stop_id, resolve_codes, station,
    Station, STATIONS,
};
pub use enrich::{enrich_trip_updates, EnrichInput};
pub use headsign::track_from_headsign;
pub use hub::TrackHub;
pub use index::{same_train, ScheduledTrip, StaticIndex};
pub use platforms::inject_platforms;

#[cfg(test)]
#[allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{Cursor, Read, Write};

    use chrono::NaiveDate;
    use gtfs_realtime::trip_update::stop_time_update::StopTimeProperties;
    use gtfs_realtime::trip_update::{StopTimeEvent, StopTimeUpdate};
    use gtfs_realtime::{FeedEntity, FeedHeader, FeedMessage, TripDescriptor, TripUpdate};
    use prost::Message;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    fn tiny_zip() -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let files = [
            (
                "stops.txt",
                "stop_id,stop_code,stop_name,stop_desc,stop_lat,stop_lon,zone_id,location_type,parent_station\n\
105,NY,NEW YORK PENN STATION,,40.75,-73.99,329,,\n\
63,HB,HOBOKEN,,40.73,-74.02,336,,\n",
            ),
            (
                "trips.txt",
                "trip_id,route_id,service_id,trip_short_name,block_id\n\
9,10,1,3846,3846\n\
10,10,2,3846,3846\n\
11,10,1,0510,0510\n",
            ),
            (
                "calendar_dates.txt",
                "service_id,date,exception_type\n1,20260115,1\n2,20260116,1\n",
            ),
            (
                "stop_times.txt",
                "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n9,10:00:00,10:00:00,105,1\n",
            ),
            (
                "agency.txt",
                "agency_id,agency_name,agency_url,agency_timezone\nNJT,NJ Transit,https://www.njtransit.com,America/New_York\n",
            ),
            (
                "routes.txt",
                "route_id,agency_id,route_short_name,route_long_name,route_type\n10,NJT,NEC,Northeast Corridor,113\n",
            ),
        ];
        for (name, contents) in files {
            writer.start_file(name, options).unwrap();
            writer.write_all(contents.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn feed(entity_id: &str, trip_id: Option<&str>, headsign: &str, delay: i32) -> Vec<u8> {
        let message = FeedMessage {
            header: FeedHeader {
                gtfs_realtime_version: "2.0".to_string(),
                incrementality: None,
                timestamp: Some(1),
                feed_version: None,
            },
            entity: vec![FeedEntity {
                id: entity_id.to_string(),
                is_deleted: None,
                trip_update: Some(TripUpdate {
                    trip: TripDescriptor {
                        trip_id: trip_id.map(str::to_string),
                        start_date: Some("20260115".to_string()),
                        schedule_relationship: Some(1),
                        ..TripDescriptor::default()
                    },
                    stop_time_update: vec![StopTimeUpdate {
                        stop_id: Some("105".to_string()),
                        departure: Some(StopTimeEvent {
                            delay: Some(delay),
                            ..StopTimeEvent::default()
                        }),
                        stop_time_properties: Some(StopTimeProperties {
                            stop_headsign: Some(headsign.to_string()),
                            ..StopTimeProperties::default()
                        }),
                        ..StopTimeUpdate::default()
                    }],
                    ..TripUpdate::default()
                }),
                ..FeedEntity::default()
            }],
        };
        message.encode_to_vec()
    }

    #[test]
    fn headsign_track_sets_assigned_stop_id_and_trip_id() {
        let zip = tiny_zip();
        let ny = station("NY").unwrap();
        let injection = inject_platforms(&zip, &[ny]).unwrap();
        let rewritten = injection.zip.expect("rewritten");
        let mut archive = ZipArchive::new(Cursor::new(rewritten)).unwrap();
        let mut stops = String::new();
        archive
            .by_name("stops.txt")
            .unwrap()
            .read_to_string(&mut stops)
            .unwrap();
        assert!(stops.contains("105,NY,NEW YORK PENN STATION"));
        assert!(stops.contains("105:station"));
        assert!(stops.contains("105:track:11"));
        assert!(stops.contains("Rail - Track 11"));
        assert!(stops.contains("NEW YORK PENN STATION - Rail - Track 11"));
        let mut times = String::new();
        archive
            .by_name("stop_times.txt")
            .unwrap()
            .read_to_string(&mut times)
            .unwrap();
        assert!(times.contains(",105,"));
        assert!(!times.contains("105:track"));

        let bytes = feed("3846", None, "Track:11", 45);
        let enriched = enrich_trip_updates(
            &bytes,
            &EnrichInput {
                stations: &[ny],
                index: &injection.index,
                boards: &HashMap::new(),
            },
        )
        .expect("changed");
        let message = FeedMessage::decode(enriched.as_slice()).unwrap();
        let update = message.entity[0].trip_update.as_ref().unwrap();
        assert_eq!(update.trip.trip_id.as_deref(), Some("9"));
        let stop = &update.stop_time_update[0];
        assert_eq!(stop.stop_id.as_deref(), Some("105"));
        assert_eq!(stop.departure.as_ref().unwrap().delay, Some(45));
        let properties = stop.stop_time_properties.as_ref().unwrap();
        assert_eq!(properties.stop_headsign.as_deref(), Some("Track:11"));
        assert_eq!(properties.assigned_stop_id.as_deref(), Some("105:track:11"));
    }

    #[test]
    fn an_existing_trip_id_and_assignment_are_kept() {
        let index = StaticIndex {
            trips: vec![ScheduledTrip {
                trip_id: "9".to_string(),
                service_id: "1".to_string(),
                train: "3846".to_string(),
            }],
            stop_stations: HashMap::from([("105".to_string(), "NY".to_string())]),
            ..StaticIndex::default()
        };
        let bytes = feed("9999", Some("already"), "Track:11", 1);
        let mut preset = FeedMessage::decode(bytes.as_slice()).unwrap();
        preset.entity[0]
            .trip_update
            .as_mut()
            .unwrap()
            .stop_time_update[0]
            .stop_time_properties
            .as_mut()
            .unwrap()
            .assigned_stop_id = Some("105:track:1".to_string());
        let bytes = preset.encode_to_vec();
        let enriched = enrich_trip_updates(
            &bytes,
            &EnrichInput {
                stations: &[station("NY").unwrap()],
                index: &index,
                boards: &HashMap::new(),
            },
        );
        assert!(enriched.is_none());
    }

    #[test]
    fn repeated_train_numbers_need_a_service_date() {
        let zip = tiny_zip();
        let index = StaticIndex::from_zip(&zip).unwrap();
        assert_eq!(index.trip_for_train("3846", Some("20260115")), Some("9"));
        assert_eq!(index.trip_for_train("3846", Some("20260116")), Some("10"));
        assert_eq!(index.trip_for_train("3846", None), None);
        assert_eq!(index.trip_for_train("510", Some("20260115")), Some("11"));
        let _ = NaiveDate::from_ymd_opt(2026, 1, 15);
    }

    #[test]
    fn single_at_summit_becomes_the_public_track() {
        let summit = station("ST").unwrap();
        assert_eq!(summit.public_track("Single"), Some("S"));
        assert_eq!(station("TS").unwrap().public_track("4"), Some("E"));
        assert_eq!(station("NA").unwrap().public_track("0"), Some("A"));
        assert_eq!(station("MP").unwrap().public_track("2"), Some("1"));
    }

    #[test]
    fn original_stop_is_not_promoted() {
        let zip = tiny_zip();
        let injection = inject_platforms(&zip, &[station("NY").unwrap()]).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(injection.zip.unwrap())).unwrap();
        let mut stops = String::new();
        archive
            .by_name("stops.txt")
            .unwrap()
            .read_to_string(&mut stops)
            .unwrap();
        let mut reader = csv::Reader::from_reader(stops.as_bytes());
        let headers = reader.headers().unwrap().clone();
        let mut saw_original = false;
        let mut saw_parent = false;
        for row in reader.records() {
            let row = row.unwrap();
            let id = row
                .get(
                    headers
                        .iter()
                        .position(|header| header == "stop_id")
                        .unwrap(),
                )
                .unwrap();
            let location = row
                .get(
                    headers
                        .iter()
                        .position(|header| header == "location_type")
                        .unwrap(),
                )
                .unwrap();
            let parent = row
                .get(
                    headers
                        .iter()
                        .position(|header| header == "parent_station")
                        .unwrap(),
                )
                .unwrap();
            if id == "105" {
                saw_original = true;
                assert_eq!(location, "");
                assert_eq!(parent, "105:station");
            }
            if id == "105:station" {
                saw_parent = true;
                assert_eq!(location, "1");
                assert!(parent.is_empty());
            }
        }
        assert!(saw_original);
        assert!(saw_parent);
    }

    #[test]
    fn a_board_track_fills_a_stop_without_a_track_headsign() {
        let ny = station("NY").unwrap();
        let rows = parse_board(
            br#"{"ITEMS":[{"TRAIN_ID":"3846","TRACK":"9","SCHED_DEP_DATE":"15-Jan-2026 10:00:00 AM","CONNECTING_TRAIN_ID":""}]}"#,
            ny,
        );
        assert_eq!(rows[0].track, "9");
        let bytes = feed("3846", Some("9"), "On time", 0);
        let mut boards = HashMap::new();
        boards.insert("NY".to_string(), rows);
        let enriched = enrich_trip_updates(
            &bytes,
            &EnrichInput {
                stations: &[ny],
                index: &StaticIndex::default(),
                boards: &boards,
            },
        )
        .expect("board track");
        let message = FeedMessage::decode(enriched.as_slice()).unwrap();
        let properties = message.entity[0]
            .trip_update
            .as_ref()
            .unwrap()
            .stop_time_update[0]
            .stop_time_properties
            .as_ref()
            .unwrap();
        assert_eq!(properties.stop_headsign.as_deref(), Some("On time"));
        assert_eq!(properties.assigned_stop_id.as_deref(), Some("105:track:9"));
    }

    #[test]
    #[ignore = "reads a local developer GTFS zip and writes /tmp/njt-gtfs/injected.zip for the validator"]
    fn write_injected_developer_zip() {
        let input = std::fs::read("/tmp/njt-gtfs/test-getgtfs.zip").expect("fixture");
        let stations: Vec<_> = STATIONS.iter().collect();
        let injection = inject_platforms(&input, &stations).expect("inject");
        let zip = injection.zip.expect("changed");
        std::fs::write("/tmp/njt-gtfs/injected.zip", &zip).expect("write");
        assert!(injection.index.trips.len() > 1000);
        assert!(injection.index.stop_stations.contains_key("105"));
    }
}
