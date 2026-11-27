//! Trip and stop indexes built from a rail GTFS zip.
//!
//! Developer `trips.txt` stores the train number in `trip_short_name` (the same
//! digits as `block_id`, sometimes with a leading zero). Added realtime trips
//! leave `trip_id` empty and put that train number on the feed entity id.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};

use zip::ZipArchive;

/// One scheduled trip that can be named from a train number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledTrip {
    /// `trips.trip_id`.
    pub trip_id: String,
    /// `trips.service_id`.
    pub service_id: String,
    /// Train number from `trip_short_name`, falling back to `block_id`.
    pub train: String,
}

/// Facts the realtime enricher needs from the static zip.
#[derive(Clone, Debug, Default)]
pub struct StaticIndex {
    /// Scheduled trips.
    pub trips: Vec<ScheduledTrip>,
    /// Dates (`YYYYMMDD`) on which each `service_id` is added.
    pub service_dates: HashMap<String, HashSet<String>>,
    /// `stop_id` to station code for catalog stations found in `stops.txt`.
    pub stop_stations: HashMap<String, String>,
}

impl StaticIndex {
    /// Reads trips, calendar dates, and catalog stops from `zip`.
    ///
    /// # Arguments
    ///
    /// * `zip` — A GTFS zip. `stops.txt` and `trips.txt` are required.
    ///
    /// # Returns
    ///
    /// The index. `calendar_dates.txt` may be absent; date filtering is then empty.
    ///
    /// # Errors
    ///
    /// Returns a message when the archive or a required file cannot be read.
    /// The message does not include feed bytes.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    ///
    /// # Examples
    ///
    /// ```
    /// use njt_gtfs_proxy::tracks::StaticIndex;
    ///
    /// assert!(StaticIndex::from_zip(b"not-a-zip").is_err());
    /// ```
    pub fn from_zip(zip: &[u8]) -> Result<Self, String> {
        let mut archive = ZipArchive::new(Cursor::new(zip)).map_err(|err| err.to_string())?;
        let trips = read_named(&mut archive, "trips.txt")?;
        let stops = read_named(&mut archive, "stops.txt")?;
        let dates = read_named(&mut archive, "calendar_dates.txt").unwrap_or_default();
        Ok(Self {
            trips: read_trips(&trips)?,
            service_dates: read_dates(&dates),
            stop_stations: read_stop_stations(&stops),
        })
    }

    /// Finds the one `trip_id` for `train` on `start_date`.
    ///
    /// # Arguments
    ///
    /// * `train` — Entity id or other train number.
    /// * `start_date` — `TripDescriptor.start_date` (`YYYYMMDD`), when the feed set it.
    ///
    /// # Returns
    ///
    /// A trip id when exactly one scheduled trip matches. Repeated train numbers
    /// without a date, or two services on the same date, return `None`.
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
    /// use njt_gtfs_proxy::tracks::{ScheduledTrip, StaticIndex};
    ///
    /// let index = StaticIndex {
    ///     trips: vec![ScheduledTrip {
    ///         trip_id: "9".to_string(),
    ///         service_id: "1".to_string(),
    ///         train: "3846".to_string(),
    ///     }],
    ///     ..StaticIndex::default()
    /// };
    /// assert_eq!(index.trip_for_train("3846", None), Some("9"));
    /// ```
    pub fn trip_for_train(&self, train: &str, start_date: Option<&str>) -> Option<&str> {
        let matches: Vec<&ScheduledTrip> = self
            .trips
            .iter()
            .filter(|trip| same_train(&trip.train, train))
            .collect();
        self.pick_trip(matches, start_date)
    }

    /// Narrows trips with the same train number to the one running on a date.
    ///
    /// # Arguments
    ///
    /// * `matches` — Trips whose train number matched.
    /// * `start_date` — `TripDescriptor.start_date` (`YYYYMMDD`), when the feed set it.
    ///
    /// # Returns
    ///
    /// The trip id when exactly one trip is left. Without a date, more than one
    /// match is ambiguous and returns `None`.
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
    /// Private helper shared by [`StaticIndex::trip_for_train`] and
    /// [`TripLookup::trip_for_train`]:
    ///
    /// ```
    /// use njt_gtfs_proxy::tracks::StaticIndex;
    ///
    /// assert_eq!(StaticIndex::default().trip_for_train("3846", None), None);
    /// ```
    fn pick_trip<'a>(
        &self,
        mut matches: Vec<&'a ScheduledTrip>,
        start_date: Option<&str>,
    ) -> Option<&'a str> {
        if let Some(date) = start_date.map(str::trim).filter(|date| !date.is_empty()) {
            matches.retain(|trip| {
                self.service_dates
                    .get(&trip.service_id)
                    .is_some_and(|dates| dates.contains(date))
            });
        } else if matches.len() > 1 {
            return None;
        }
        if matches.len() == 1 {
            Some(matches[0].trip_id.as_str())
        } else {
            None
        }
    }
}

/// Hash lookups over a [`StaticIndex`] for one trip-update feed.
///
/// [`StaticIndex::trip_for_train`] scans every trip. A feed has hundreds of
/// entities and the index has thousands of trips, so the enricher builds this
/// once per feed and then finds each entity's trip in constant time.
#[derive(Debug)]
pub struct TripLookup<'a> {
    /// The index the maps point into.
    index: &'a StaticIndex,
    /// `trip_id` to its trip. The first row wins when an id repeats.
    by_id: HashMap<&'a str, &'a ScheduledTrip>,
    /// [`train_key`] to every trip with that train number, in index order.
    by_train: HashMap<String, Vec<&'a ScheduledTrip>>,
}

impl<'a> TripLookup<'a> {
    /// Builds the maps for `index`.
    ///
    /// # Arguments
    ///
    /// * `index` — The static index to look into.
    ///
    /// # Returns
    ///
    /// The lookup.
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
    /// use njt_gtfs_proxy::tracks::{StaticIndex, TripLookup};
    ///
    /// let index = StaticIndex::default();
    /// assert!(TripLookup::new(&index).trip("9").is_none());
    /// ```
    pub fn new(index: &'a StaticIndex) -> Self {
        let mut by_id = HashMap::new();
        let mut by_train: HashMap<String, Vec<&'a ScheduledTrip>> = HashMap::new();
        for trip in &index.trips {
            by_id.entry(trip.trip_id.as_str()).or_insert(trip);
            if let Some(key) = train_key(&trip.train) {
                by_train.entry(key).or_default().push(trip);
            }
        }
        Self {
            index,
            by_id,
            by_train,
        }
    }

    /// Finds a scheduled trip by `trip_id`.
    ///
    /// # Arguments
    ///
    /// * `trip_id` — `trips.trip_id`.
    ///
    /// # Returns
    ///
    /// The trip, or `None` when the index does not have it.
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
    /// use njt_gtfs_proxy::tracks::{ScheduledTrip, StaticIndex, TripLookup};
    ///
    /// let index = StaticIndex {
    ///     trips: vec![ScheduledTrip {
    ///         trip_id: "9".to_string(),
    ///         service_id: "1".to_string(),
    ///         train: "3846".to_string(),
    ///     }],
    ///     ..StaticIndex::default()
    /// };
    /// assert_eq!(TripLookup::new(&index).trip("9").map(|trip| trip.train.as_str()), Some("3846"));
    /// ```
    pub fn trip(&self, trip_id: &str) -> Option<&'a ScheduledTrip> {
        self.by_id.get(trip_id).copied()
    }

    /// Same result as [`StaticIndex::trip_for_train`], without a full scan.
    ///
    /// # Arguments
    ///
    /// * `train` — Entity id or other train number.
    /// * `start_date` — `TripDescriptor.start_date` (`YYYYMMDD`), when the feed set it.
    ///
    /// # Returns
    ///
    /// A trip id when exactly one scheduled trip matches.
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
    /// use njt_gtfs_proxy::tracks::{ScheduledTrip, StaticIndex, TripLookup};
    ///
    /// let index = StaticIndex {
    ///     trips: vec![ScheduledTrip {
    ///         trip_id: "11".to_string(),
    ///         service_id: "1".to_string(),
    ///         train: "0510".to_string(),
    ///     }],
    ///     ..StaticIndex::default()
    /// };
    /// assert_eq!(TripLookup::new(&index).trip_for_train("510", None), Some("11"));
    /// ```
    pub fn trip_for_train(&self, train: &str, start_date: Option<&str>) -> Option<&'a str> {
        let key = train_key(train)?;
        let matches = self.by_train.get(&key).cloned().unwrap_or_default();
        self.index.pick_trip(matches, start_date)
    }
}

/// Normalizes a train number so equal keys mean [`same_train`] is true.
///
/// # Arguments
///
/// * `train` — A `trip_short_name`, `block_id`, or entity id.
///
/// # Returns
///
/// `None` for a blank value. An integer becomes its decimal value without
/// leading zeros. Anything else is trimmed and upper-cased.
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
/// Private helper. Its keys agree with [`same_train`]:
///
/// ```
/// use njt_gtfs_proxy::tracks::same_train;
///
/// assert!(same_train("0510", "510"));
/// assert!(same_train("a125", "A125"));
/// ```
fn train_key(train: &str) -> Option<String> {
    let train = train.trim();
    if train.is_empty() {
        return None;
    }
    Some(match train.parse::<u32>() {
        Ok(number) => number.to_string(),
        Err(_) => train.to_ascii_uppercase(),
    })
}

/// Reports whether two train numbers are the same, ignoring leading zeros.
///
/// # Arguments
///
/// * `left` — A `trip_short_name`, `block_id`, or entity id.
/// * `right` — The other number.
///
/// # Returns
///
/// `true` when the trimmed strings match, or both are integers with the same value.
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
/// assert!(!same_train("A125", "125"));
/// ```
pub fn same_train(left: &str, right: &str) -> bool {
    let left = left.trim();
    let right = right.trim();
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left.eq_ignore_ascii_case(right) {
        return true;
    }
    matches!(
        (left.parse::<u32>(), right.parse::<u32>()),
        (Ok(left), Ok(right)) if left == right
    )
}

/// Reads a zip entry, or fails when it is missing.
///
/// # Arguments
///
/// * `archive` — Open GTFS archive.
/// * `name` — File name at the archive root, or in one directory.
///
/// # Returns
///
/// The file bytes.
///
/// # Errors
///
/// Returns a message when the entry is absent or cannot be read.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::StaticIndex;
///
/// assert!(StaticIndex::from_zip(b"PK\x03\x04").is_err());
/// ```
fn read_named<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>, String> {
    let mut file = archive
        .by_name(name)
        .map_err(|_| format!("missing {name}"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    Ok(bytes)
}

/// Builds scheduled trips from `trips.txt`.
///
/// # Arguments
///
/// * `bytes` — CSV bytes, optionally starting with a UTF-8 BOM.
///
/// # Returns
///
/// One record per data row that has a trip id and a train number.
///
/// # Errors
///
/// Returns a message when the CSV cannot be parsed or has no `trip_id`.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// The reader is covered by [`StaticIndex::trip_for_train`].
///
/// ```
/// use njt_gtfs_proxy::tracks::same_train;
///
/// assert!(same_train("3846", "3846"));
/// ```
fn read_trips(bytes: &[u8]) -> Result<Vec<ScheduledTrip>, String> {
    let mut reader = csv_reader(bytes);
    let headers = reader
        .headers()
        .map_err(|err| err.to_string())?
        .iter()
        .map(|header| header.trim().to_string())
        .collect::<Vec<_>>();
    let column = |name: &str| headers.iter().position(|header| header == name);
    let trip_id = column("trip_id").ok_or_else(|| "trips.txt has no trip_id".to_string())?;
    let service_id = column("service_id");
    let short_name = column("trip_short_name");
    let block_id = column("block_id");
    let mut trips = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|err| err.to_string())?;
        let Some(id) = record
            .get(trip_id)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let train = short_name
            .and_then(|index| record.get(index))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                block_id
                    .and_then(|index| record.get(index))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            });
        let Some(train) = train else {
            continue;
        };
        trips.push(ScheduledTrip {
            trip_id: id.to_string(),
            service_id: service_id
                .and_then(|index| record.get(index))
                .unwrap_or("")
                .trim()
                .to_string(),
            train: train.to_string(),
        });
    }
    Ok(trips)
}

/// Reads `calendar_dates.txt` additions.
///
/// # Arguments
///
/// * `bytes` — CSV bytes. Empty input yields an empty map.
///
/// # Returns
///
/// Service ids mapped to the dates with `exception_type` 1. Other exception
/// types are ignored. A feed that only lists added dates, which is what the
/// developer rail zip does, is the case this supports.
///
/// # Errors
///
/// This function does not return an error. A broken CSV yields an empty map.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::StaticIndex;
///
/// let index = StaticIndex::default();
/// assert!(index.service_dates.is_empty());
/// ```
fn read_dates(bytes: &[u8]) -> HashMap<String, HashSet<String>> {
    let mut map = HashMap::<String, HashSet<String>>::new();
    if bytes.is_empty() {
        return map;
    }
    let mut reader = csv_reader(bytes);
    let Ok(headers) = reader.headers() else {
        return map;
    };
    let headers: Vec<String> = headers
        .iter()
        .map(|header| header.trim().to_string())
        .collect();
    let Some(service) = headers.iter().position(|header| header == "service_id") else {
        return map;
    };
    let Some(date) = headers.iter().position(|header| header == "date") else {
        return map;
    };
    let exception = headers.iter().position(|header| header == "exception_type");
    for record in reader.records().flatten() {
        if exception.is_some_and(|index| record.get(index).unwrap_or("").trim() != "1") {
            continue;
        }
        let Some(service_id) = record
            .get(service)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let Some(day) = record
            .get(date)
            .map(str::trim)
            .filter(|day| !day.is_empty())
        else {
            continue;
        };
        map.entry(service_id.to_string())
            .or_default()
            .insert(day.to_string());
    }
    map
}

/// Maps catalog station stop codes and known stop ids onto `stop_id`.
///
/// # Arguments
///
/// * `bytes` — `stops.txt` bytes.
///
/// # Returns
///
/// `stop_id` to station code. A row is included when its `stop_code` is a
/// catalog code and its name contains that station's fragment, or when its
/// `stop_id` is the catalog id and the name matches.
///
/// # Errors
///
/// This function does not return an error. A broken CSV yields an empty map.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::StaticIndex;
///
/// assert!(StaticIndex::default().stop_stations.is_empty());
/// ```
fn read_stop_stations(bytes: &[u8]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut reader = csv_reader(bytes);
    let Ok(headers) = reader.headers() else {
        return map;
    };
    let headers: Vec<String> = headers
        .iter()
        .map(|header| header.trim().to_string())
        .collect();
    let Some(stop_id) = headers.iter().position(|header| header == "stop_id") else {
        return map;
    };
    let stop_code = headers.iter().position(|header| header == "stop_code");
    let stop_name = headers.iter().position(|header| header == "stop_name");
    for record in reader.records().flatten() {
        let Some(id) = record
            .get(stop_id)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let name = stop_name
            .and_then(|index| record.get(index))
            .unwrap_or("")
            .to_ascii_uppercase();
        let code = stop_code
            .and_then(|index| record.get(index))
            .unwrap_or("")
            .trim();
        if let Some(station) = super::catalog::station(code) {
            if name.contains(station.name_fragment) {
                map.insert(id.to_string(), station.code.to_string());
                continue;
            }
        }
        for station in super::catalog::STATIONS {
            if station.stop_id == id && name.contains(station.name_fragment) {
                map.insert(id.to_string(), station.code.to_string());
                break;
            }
        }
    }
    map
}

/// Builds a CSV reader that accepts short rows and a leading BOM.
///
/// # Arguments
///
/// * `bytes` — File contents.
///
/// # Returns
///
/// A flexible CSV reader positioned at the header.
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
/// assert!(!same_train("", "1"));
/// ```
fn csv_reader(bytes: &[u8]) -> csv::Reader<&[u8]> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    csv::ReaderBuilder::new().flexible(true).from_reader(bytes)
}
