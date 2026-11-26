//! NJ Transit rail fares added to the static GTFS zip.
//!
//! The checked-in tables in `data/fares/` come from NJ Transit's 2026 rail
//! fare PDFs. [`augment_rail_gtfs`] copies those fares into a rail GTFS zip as
//! Fares v2 files plus legacy Fares v1 `fare_attributes.txt` and
//! `fare_rules.txt`. One-way prices do not vary by time of day, so the zip has
//! no `timeframes.txt`. A transfer inside the rail network stays one
//! origin-to-destination fare, expressed with `fare_leg_join_rules.txt`.
//!
//! # Examples
//!
//! A body that is not a GTFS zip is returned unchanged:
//!
//! ```
//! use njt_gtfs_proxy::augment_rail_gtfs;
//!
//! let augmented = augment_rail_gtfs(b"not-a-zip").expect("passthrough");
//! assert!(!augmented.fares_added);
//! assert_eq!(augmented.bytes, b"not-a-zip");
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Cursor, Read, Write};
use std::sync::OnceLock;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// Checked-in fare rows.
const FARES_CSV: &str = include_str!("../../../data/fares/fares.csv");
/// Checked-in stop-to-area rows.
const STATIONS_CSV: &str = include_str!("../../../data/fares/stations.csv");

/// Header set on `GET /rail/gtfs.zip` to report whether fare files were added.
pub const FARE_HEADER: &str = "x-njt-fares";

/// [`FARE_HEADER`] value when fare files were written.
pub const FARE_HEADER_ADDED: &str = "added";

/// [`FARE_HEADER`] value when the body was left unchanged.
pub const FARE_HEADER_PASSTHROUGH: &str = "passthrough";

/// Network id for NJ Transit heavy rail.
const NETWORK_ID: &str = "njt_rail";

/// Files replaced on each augment so an upstream fare draft cannot linger.
const REPLACED_FILES: &[&str] = &[
    "stops.txt",
    "areas.txt",
    "stop_areas.txt",
    "networks.txt",
    "route_networks.txt",
    "rider_categories.txt",
    "fare_media.txt",
    "fare_products.txt",
    "fare_leg_rules.txt",
    "fare_leg_join_rules.txt",
    "fare_transfer_rules.txt",
    "timeframes.txt",
    "fare_attributes.txt",
    "fare_rules.txt",
];

/// A rail GTFS zip after the fare step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AugmentedGtfs {
    /// Zip bytes to serve.
    pub bytes: Vec<u8>,
    /// Whether fare files and priced `stops.zone_id` values were written.
    pub fares_added: bool,
}

/// Failure while reading or repacking a rail GTFS zip.
#[derive(Debug)]
pub struct FareError {
    /// Operator-facing detail. It does not include credentials.
    detail: String,
}

impl std::fmt::Display for FareError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "rail fare tables: {}", self.detail)
    }
}

impl std::error::Error for FareError {}

impl FareError {
    /// Builds an error from text that is safe to log.
    ///
    /// # Arguments
    ///
    /// * `detail` — What failed.
    ///
    /// # Returns
    ///
    /// The error.
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
    /// Passthrough of a non-zip body does not use this error:
    ///
    /// ```
    /// use njt_gtfs_proxy::augment_rail_gtfs;
    ///
    /// assert!(augment_rail_gtfs(b"not-a-zip").is_ok());
    /// ```
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

/// One priced pair from the checked-in fare table.
#[derive(Debug, Clone)]
struct Fare {
    /// Chart terminal area.
    origin_area_id: String,
    /// Destination zone or station area.
    destination_area_id: String,
    /// Display name of the origin area.
    origin_name: String,
    /// Display name of the destination area.
    destination_name: String,
    /// Adult one-way amount, with two decimal places.
    adult_ow: String,
    /// Child, senior, and disabled one-way amount.
    reduced_ow: String,
    /// Ten-trip amount. Empty on the Atlantic City chart.
    ten_trip: String,
    /// Weekly pass amount.
    weekly: String,
    /// FlexPass amount.
    flexpass: String,
    /// Monthly pass amount.
    monthly: String,
    /// School monthly amount.
    school_monthly: String,
}

/// Fare area for one GTFS stop.
#[derive(Debug, Clone)]
struct StationArea {
    /// GTFS `stop_id`.
    stop_id: String,
    /// Area id. Also written to `stops.zone_id` for Fares v1.
    area_id: String,
    /// Rider-facing area name.
    area_name: String,
}

/// Checked-in fares and the stops they price.
#[derive(Debug)]
struct Catalog {
    /// Priced pairs, one source direction each.
    fares: Vec<Fare>,
    /// `stop_id` to fare area.
    stations: HashMap<String, StationArea>,
}

/// Embedded catalog, parsed once.
///
/// # Returns
///
/// The checked-in fare catalog.
///
/// # Errors
///
/// This function does not return an error.
///
/// # Panics
///
/// Panics if the checked-in fare CSV cannot be parsed.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(!augment_rail_gtfs(b"x").unwrap().fares_added);
/// ```
fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        Catalog::parse(FARES_CSV, STATIONS_CSV).expect("checked-in rail fare tables parse")
    })
}

impl Catalog {
    /// Parses fare and station CSV text.
    ///
    /// # Arguments
    ///
    /// * `fares_csv` — Contents of `fares.csv`.
    /// * `stations_csv` — Contents of `stations.csv`.
    ///
    /// # Returns
    ///
    /// The catalog.
    ///
    /// # Errors
    ///
    /// Returns [`FareError`] when the CSV is malformed or a required column is
    /// missing.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    ///
    /// # Examples
    ///
    /// Parsing is what [`augment_rail_gtfs`] does on first use:
    ///
    /// ```
    /// use njt_gtfs_proxy::augment_rail_gtfs;
    ///
    /// assert!(!augment_rail_gtfs(b"x").unwrap().fares_added);
    /// ```
    fn parse(fares_csv: &str, stations_csv: &str) -> Result<Self, FareError> {
        let fares = read_fares(fares_csv)?;
        let stations = read_stations(stations_csv)?;
        Ok(Self { fares, stations })
    }
}

/// Reads `fares.csv`.
///
/// # Arguments
///
/// * `text` — CSV text including a header.
///
/// # Returns
///
/// One [`Fare`] per data row.
///
/// # Errors
///
/// Returns [`FareError`] when the CSV cannot be read or a column is missing.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn read_fares(text: &str) -> Result<Vec<Fare>, FareError> {
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    let headers = reader
        .headers()
        .map_err(|err| FareError::new(err.to_string()))?
        .clone();
    let mut fares = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|err| FareError::new(err.to_string()))?;
        fares.push(Fare {
            origin_area_id: column(&headers, &record, "origin_area_id")?,
            destination_area_id: column(&headers, &record, "destination_area_id")?,
            origin_name: column(&headers, &record, "origin_name")?,
            destination_name: column(&headers, &record, "destination_name")?,
            adult_ow: column(&headers, &record, "adult_ow")?,
            reduced_ow: column(&headers, &record, "reduced_ow")?,
            ten_trip: column(&headers, &record, "ten_trip")?,
            weekly: column(&headers, &record, "weekly")?,
            flexpass: column(&headers, &record, "flexpass")?,
            monthly: column(&headers, &record, "monthly")?,
            school_monthly: column(&headers, &record, "school_monthly")?,
        });
    }
    Ok(fares)
}

/// Reads `stations.csv` into a stop-id map.
///
/// # Arguments
///
/// * `text` — CSV text including a header.
///
/// # Returns
///
/// Stop id to [`StationArea`].
///
/// # Errors
///
/// Returns [`FareError`] when the CSV cannot be read or a column is missing.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn read_stations(text: &str) -> Result<HashMap<String, StationArea>, FareError> {
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    let headers = reader
        .headers()
        .map_err(|err| FareError::new(err.to_string()))?
        .clone();
    let mut stations = HashMap::new();
    for record in reader.records() {
        let record = record.map_err(|err| FareError::new(err.to_string()))?;
        let station = StationArea {
            stop_id: column(&headers, &record, "stop_id")?,
            area_id: column(&headers, &record, "area_id")?,
            area_name: column(&headers, &record, "area_name")?,
        };
        stations.insert(station.stop_id.clone(), station);
    }
    Ok(stations)
}

/// Copies one named column from a CSV record.
///
/// # Arguments
///
/// * `headers` — Header row.
/// * `record` — Data row.
/// * `name` — Column name.
///
/// # Returns
///
/// The field, or an empty string when the row is shorter than the header.
///
/// # Errors
///
/// Returns [`FareError`] when `name` is not in `headers`.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn column(
    headers: &csv::StringRecord,
    record: &csv::StringRecord,
    name: &str,
) -> Result<String, FareError> {
    let index = headers
        .iter()
        .position(|header| header == name)
        .ok_or_else(|| FareError::new(format!("fare table is missing column {name}")))?;
    Ok(record.get(index).unwrap_or("").to_string())
}

/// Adds rail fare files to a GTFS zip.
///
/// Stops that the fare table prices have `zone_id` set to their fare area so
/// Fares v1 `fare_rules.origin_id` and `destination_id` match. Other stops,
/// including light rail, keep the upstream `zone_id`. A body that is not a zip
/// or has no `stops.txt` is returned unchanged.
///
/// # Arguments
///
/// * `zip_bytes` — Upstream rail GTFS zip, or any other body.
///
/// # Returns
///
/// The bytes to serve and whether fare files were written.
///
/// # Errors
///
/// Returns [`FareError`] when `stops.txt` exists but the zip cannot be read or
/// rewritten. The message does not include credentials.
///
/// # Panics
///
/// Panics if the checked-in fare CSV cannot be parsed. That CSV is part of
/// this crate.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// let augmented = augment_rail_gtfs(b"not-a-zip").expect("passthrough");
/// assert!(!augmented.fares_added);
/// ```
pub fn augment_rail_gtfs(zip_bytes: &[u8]) -> Result<AugmentedGtfs, FareError> {
    let cursor = Cursor::new(zip_bytes);
    let mut archive = match ZipArchive::new(cursor) {
        Ok(archive) => archive,
        Err(_) => {
            return Ok(AugmentedGtfs {
                bytes: zip_bytes.to_vec(),
                fares_added: false,
            });
        }
    };
    if archive.by_name("stops.txt").is_err() {
        return Ok(AugmentedGtfs {
            bytes: zip_bytes.to_vec(),
            fares_added: false,
        });
    }
    let mut files = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| FareError::new(err.to_string()))?;
        let name = file
            .enclosed_name()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| file.name().to_string());
        if name.ends_with('/') {
            continue;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|err| FareError::new(err.to_string()))?;
        files.insert(name, bytes);
    }
    let stops = files
        .get("stops.txt")
        .map(Vec::as_slice)
        .ok_or_else(|| FareError::new("stops.txt disappeared while reading the zip"))?;
    let routes = files.get("routes.txt").map(Vec::as_slice);
    let generated = generate(catalog(), stops, routes)?;
    for name in REPLACED_FILES {
        files.remove(*name);
    }
    files.insert("stops.txt".to_string(), generated.stops);
    for (name, bytes) in generated.files {
        files.insert(name, bytes);
    }
    Ok(AugmentedGtfs {
        bytes: write_zip(&files)?,
        fares_added: true,
    })
}

/// Reports whether `zip_bytes` already contains `fare_rules.txt`.
///
/// # Arguments
///
/// * `zip_bytes` — A response body.
///
/// # Returns
///
/// `true` when the zip has a `fare_rules.txt` entry.
///
/// # Errors
///
/// This function does not return an error. An unreadable body is `false`.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::fares::zip_has_fare_rules;
///
/// assert!(!zip_has_fare_rules(b"not-a-zip"));
/// ```
pub fn zip_has_fare_rules(zip_bytes: &[u8]) -> bool {
    let Ok(mut archive) = ZipArchive::new(Cursor::new(zip_bytes)) else {
        return false;
    };
    let found = archive.by_name("fare_rules.txt").is_ok();
    found
}

/// Generated fare files plus the rewritten stop table.
struct Generated {
    /// `stops.txt` with priced `zone_id` values.
    stops: Vec<u8>,
    /// New fare files, keyed by file name.
    files: BTreeMap<String, Vec<u8>>,
}

/// Builds fare files for the stops and routes in this zip.
///
/// # Arguments
///
/// * `catalog` — Checked-in fares.
/// * `stops_txt` — Upstream `stops.txt` bytes.
/// * `routes_txt` — Upstream `routes.txt` bytes, when the zip has that file.
///
/// # Returns
///
/// The rewritten stops and the fare files.
///
/// # Errors
///
/// Returns [`FareError`] when `stops.txt` or `routes.txt` is not CSV.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// Generation runs inside [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn generate(
    catalog: &Catalog,
    stops_txt: &[u8],
    routes_txt: Option<&[u8]>,
) -> Result<Generated, FareError> {
    let (stops_header, stop_rows) = read_table(stops_txt, "stops.txt")?;
    let zone_column = stops_header
        .iter()
        .position(|name| name == "zone_id")
        .ok_or_else(|| FareError::new("stops.txt has no zone_id column"))?;
    let id_column = stops_header
        .iter()
        .position(|name| name == "stop_id")
        .ok_or_else(|| FareError::new("stops.txt has no stop_id column"))?;

    let mut present_areas: BTreeMap<String, String> = BTreeMap::new();
    let mut stop_areas: Vec<(String, String)> = Vec::new();
    let mut rewritten = Vec::new();
    for row in stop_rows {
        let mut row = row;
        let matched = row
            .get(id_column)
            .and_then(|stop_id| catalog.stations.get(stop_id))
            .map(|station| {
                (
                    station.stop_id.clone(),
                    station.area_id.clone(),
                    station.area_name.clone(),
                )
            });
        if let Some((stop_id, area_id, area_name)) = matched {
            if row.len() > zone_column {
                row[zone_column].clone_from(&area_id);
            }
            present_areas.entry(area_id.clone()).or_insert(area_name);
            stop_areas.push((stop_id, area_id));
        }
        rewritten.push(row);
    }

    let mut usable = Vec::new();
    for fare in &catalog.fares {
        if present_areas.contains_key(&fare.origin_area_id)
            && present_areas.contains_key(&fare.destination_area_id)
            && fare.origin_area_id != fare.destination_area_id
        {
            usable.push(fare);
        }
    }

    let mut files = BTreeMap::new();
    files.insert(
        "areas.txt".to_string(),
        write_records(
            &["area_id", "area_name"],
            present_areas
                .iter()
                .map(|(id, name)| vec![id.clone(), name.clone()])
                .collect(),
        )?,
    );
    files.insert(
        "stop_areas.txt".to_string(),
        write_records(
            &["stop_id", "area_id"],
            stop_areas
                .iter()
                .map(|(stop, area)| vec![stop.clone(), area.clone()])
                .collect(),
        )?,
    );
    files.insert(
        "networks.txt".to_string(),
        write_records(
            &["network_id", "network_name"],
            vec![vec![NETWORK_ID.to_string(), "NJ TRANSIT Rail".to_string()]],
        )?,
    );
    files.insert(
        "route_networks.txt".to_string(),
        write_records(&["network_id", "route_id"], rail_route_rows(routes_txt)?)?,
    );
    files.insert("rider_categories.txt".to_string(), rider_categories()?);
    files.insert("fare_media.txt".to_string(), fare_media()?);
    files.insert(
        "fare_leg_join_rules.txt".to_string(),
        write_records(
            &[
                "from_network_id",
                "to_network_id",
                "from_stop_id",
                "to_stop_id",
            ],
            vec![vec![
                NETWORK_ID.to_string(),
                NETWORK_ID.to_string(),
                String::new(),
                String::new(),
            ]],
        )?,
    );

    let products = products_for(&usable);
    files.insert("fare_products.txt".to_string(), product_file(&products)?);
    files.insert("fare_leg_rules.txt".to_string(), leg_rules(&products)?);
    files.insert(
        "fare_attributes.txt".to_string(),
        fare_attributes(&products)?,
    );
    files.insert("fare_rules.txt".to_string(), fare_rules(&products)?);

    Ok(Generated {
        stops: write_records(
            &stops_header.iter().map(String::as_str).collect::<Vec<_>>(),
            rewritten,
        )?,
        files,
    })
}

/// One sellable fare product for an undirected area pair.
struct Product {
    /// Stable id shared by both directions.
    id: String,
    /// Rider-facing name.
    name: String,
    /// `rider_categories.rider_category_id`.
    rider: &'static str,
    /// `fare_media.fare_media_id`.
    media: &'static str,
    /// Decimal amount.
    amount: String,
    /// Origin area.
    from_area: String,
    /// Destination area.
    to_area: String,
    /// Whether Fares v1 should publish this product. Adult one-way only.
    legacy: bool,
}

/// Builds the product list for fares whose areas are both in this zip.
///
/// # Arguments
///
/// * `fares` — Pairs that apply to stops in the zip.
///
/// # Returns
///
/// Products sorted by id, rider, and media.
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
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn products_for(fares: &[&Fare]) -> Vec<Product> {
    let mut products = BTreeMap::new();
    for fare in fares {
        for kind in kinds(fare) {
            let id = pair_id(kind.suffix, &fare.origin_area_id, &fare.destination_area_id);
            let key = format!("{id}|{}|{}", kind.rider, kind.media);
            products.entry(key).or_insert(Product {
                id,
                name: format!(
                    "{} between {} and {}",
                    kind.label, fare.origin_name, fare.destination_name
                ),
                rider: kind.rider,
                media: kind.media,
                amount: kind.amount,
                from_area: fare.origin_area_id.clone(),
                to_area: fare.destination_area_id.clone(),
                legacy: kind.legacy,
            });
        }
    }
    products.into_values().collect()
}

/// A column of the fare table turned into a GTFS product kind.
struct Kind {
    /// Id prefix.
    suffix: &'static str,
    /// Name prefix.
    label: &'static str,
    /// Rider category.
    rider: &'static str,
    /// Fare media.
    media: &'static str,
    /// Amount text.
    amount: String,
    /// Include in Fares v1.
    legacy: bool,
}

/// Lists the non-empty products on one fare row.
///
/// Round-trip columns stay in `fares.csv` and are not GTFS leg prices. A leg
/// is one way; publishing the round-trip amount as a leg would overstate it.
///
/// # Arguments
///
/// * `fare` — One checked-in pair.
///
/// # Returns
///
/// The products whose amount is non-empty.
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
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn kinds(fare: &Fare) -> Vec<Kind> {
    let mut kinds = vec![Kind {
        suffix: "ow_adult",
        label: "Adult one-way",
        rider: "adult",
        media: "njt_ticket",
        amount: fare.adult_ow.clone(),
        legacy: true,
    }];
    for (suffix, label, rider) in [
        ("ow_child", "Child one-way", "child"),
        ("ow_senior", "Senior one-way", "senior"),
        ("ow_disabled", "Disabled one-way", "disabled"),
    ] {
        kinds.push(Kind {
            suffix,
            label,
            rider,
            media: "njt_ticket",
            amount: fare.reduced_ow.clone(),
            legacy: false,
        });
    }
    for (suffix, label, media, amount) in [
        ("ten_trip", "10-trip", "njt_ten_trip", fare.ten_trip.clone()),
        ("weekly", "Weekly pass", "njt_weekly", fare.weekly.clone()),
        (
            "flexpass",
            "FlexPass",
            "njt_flexpass",
            fare.flexpass.clone(),
        ),
        (
            "monthly",
            "Monthly pass",
            "njt_monthly",
            fare.monthly.clone(),
        ),
        (
            "school_monthly",
            "School monthly",
            "njt_school_monthly",
            fare.school_monthly.clone(),
        ),
    ] {
        let rider = if suffix == "school_monthly" {
            "student"
        } else {
            "adult"
        };
        kinds.push(Kind {
            suffix,
            label,
            rider,
            media,
            amount,
            legacy: false,
        });
    }
    kinds.retain(|kind| !kind.amount.is_empty());
    kinds
}

/// Builds an undirected product id from two area ids.
///
/// # Arguments
///
/// * `suffix` — Product kind.
/// * `left` — One area id.
/// * `right` — The other area id.
///
/// # Returns
///
/// An id that does not change if the areas are swapped.
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
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn pair_id(suffix: &str, left: &str, right: &str) -> String {
    if left <= right {
        format!("{suffix}_{left}_{right}")
    } else {
        format!("{suffix}_{right}_{left}")
    }
}

/// `rider_categories.txt` for the products this module emits.
///
/// Adult is the default category. Child, senior, and disabled share the
/// reduced one-way amount from the fare chart. Student is the school monthly
/// only.
///
/// # Returns
///
/// CSV bytes.
///
/// # Errors
///
/// Returns [`FareError`] when the CSV writer fails.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn rider_categories() -> Result<Vec<u8>, FareError> {
    write_records(
        &[
            "rider_category_id",
            "rider_category_name",
            "is_default_fare_category",
        ],
        vec![
            vec!["adult".into(), "Adult".into(), "1".into()],
            vec!["child".into(), "Child (ages 5-11)".into(), "0".into()],
            vec!["senior".into(), "Senior (62 and older)".into(), "0".into()],
            vec!["disabled".into(), "Disabled".into(), "0".into()],
            vec!["student".into(), "Student".into(), "0".into()],
        ],
    )
}

/// `fare_media.txt` for tickets and passes.
///
/// The one-way ticket uses media type 4 (mobile app). Paper vending-machine
/// tickets are the same price; pass products use type 1.
///
/// # Returns
///
/// CSV bytes.
///
/// # Errors
///
/// Returns [`FareError`] when the CSV writer fails.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn fare_media() -> Result<Vec<u8>, FareError> {
    write_records(
        &["fare_media_id", "fare_media_name", "fare_media_type"],
        vec![
            vec![
                "njt_ticket".into(),
                "NJ TRANSIT ticket (app or vending machine)".into(),
                "4".into(),
            ],
            vec!["njt_ten_trip".into(), "10-trip ticket".into(), "1".into()],
            vec!["njt_weekly".into(), "Weekly pass".into(), "1".into()],
            vec!["njt_flexpass".into(), "FlexPass".into(), "1".into()],
            vec!["njt_monthly".into(), "Monthly pass".into(), "1".into()],
            vec![
                "njt_school_monthly".into(),
                "School monthly pass".into(),
                "1".into(),
            ],
        ],
    )
}

/// Writes `fare_products.txt`.
///
/// # Arguments
///
/// * `products` — Products to publish.
///
/// # Returns
///
/// CSV bytes.
///
/// # Errors
///
/// Returns [`FareError`] when the CSV writer fails.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn product_file(products: &[Product]) -> Result<Vec<u8>, FareError> {
    write_records(
        &[
            "fare_product_id",
            "fare_product_name",
            "rider_category_id",
            "fare_media_id",
            "amount",
            "currency",
        ],
        products
            .iter()
            .map(|product| {
                vec![
                    product.id.clone(),
                    product.name.clone(),
                    product.rider.to_string(),
                    product.media.to_string(),
                    product.amount.clone(),
                    "USD".to_string(),
                ]
            })
            .collect(),
    )
}

/// Writes `fare_leg_rules.txt` in both directions.
///
/// # Arguments
///
/// * `products` — Products, each tied to one area pair.
///
/// # Returns
///
/// CSV bytes.
///
/// # Errors
///
/// Returns [`FareError`] when the CSV writer fails.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn leg_rules(products: &[Product]) -> Result<Vec<u8>, FareError> {
    let mut rows = Vec::new();
    for product in products {
        for (from, to) in [
            (&product.from_area, &product.to_area),
            (&product.to_area, &product.from_area),
        ] {
            rows.push(vec![
                NETWORK_ID.to_string(),
                NETWORK_ID.to_string(),
                from.clone(),
                to.clone(),
                product.id.clone(),
            ]);
        }
    }
    rows.sort();
    rows.dedup();
    write_records(
        &[
            "leg_group_id",
            "network_id",
            "from_area_id",
            "to_area_id",
            "fare_product_id",
        ],
        rows,
    )
}

/// Writes Fares v1 `fare_attributes.txt` for adult one-way products.
///
/// Payment is before boarding. `transfers` is empty, which means the fare does
/// not add a transfer limit of its own: the ticket is the origin-to-destination
/// price, including a rail-to-rail transfer such as Secaucus.
///
/// # Arguments
///
/// * `products` — All products. Only `legacy` rows are written.
///
/// # Returns
///
/// CSV bytes.
///
/// # Errors
///
/// Returns [`FareError`] when the CSV writer fails.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn fare_attributes(products: &[Product]) -> Result<Vec<u8>, FareError> {
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for product in products.iter().filter(|product| product.legacy) {
        if !seen.insert(product.id.clone()) {
            continue;
        }
        rows.push(vec![
            product.id.clone(),
            product.amount.clone(),
            "USD".to_string(),
            "1".to_string(),
            String::new(),
            "NJT".to_string(),
            String::new(),
        ]);
    }
    write_records(
        &[
            "fare_id",
            "price",
            "currency_type",
            "payment_method",
            "transfers",
            "agency_id",
            "transfer_duration",
        ],
        rows,
    )
}

/// Writes Fares v1 `fare_rules.txt` using area ids as zone ids.
///
/// # Arguments
///
/// * `products` — All products. Only adult one-way rows become rules.
///
/// # Returns
///
/// CSV bytes.
///
/// # Errors
///
/// Returns [`FareError`] when the CSV writer fails.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn fare_rules(products: &[Product]) -> Result<Vec<u8>, FareError> {
    let mut rows = Vec::new();
    for product in products.iter().filter(|product| product.legacy) {
        rows.push(vec![
            product.id.clone(),
            product.from_area.clone(),
            product.to_area.clone(),
        ]);
        rows.push(vec![
            product.id.clone(),
            product.to_area.clone(),
            product.from_area.clone(),
        ]);
    }
    rows.sort();
    rows.dedup();
    write_records(&["fare_id", "origin_id", "destination_id"], rows)
}

/// Route rows for heavy rail (`route_type` 2).
///
/// # Arguments
///
/// * `routes_txt` — Upstream `routes.txt`, if the zip has one.
///
/// # Returns
///
/// `network_id, route_id` rows. Light rail routes are omitted.
///
/// # Errors
///
/// Returns [`FareError`] when `routes.txt` is not CSV or has no `route_id` or
/// `route_type` column.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn rail_route_rows(routes_txt: Option<&[u8]>) -> Result<Vec<Vec<String>>, FareError> {
    let Some(bytes) = routes_txt else {
        return Ok(Vec::new());
    };
    let (header, rows) = read_table(bytes, "routes.txt")?;
    let id_column = header
        .iter()
        .position(|name| name == "route_id")
        .ok_or_else(|| FareError::new("routes.txt has no route_id column"))?;
    let type_column = header
        .iter()
        .position(|name| name == "route_type")
        .ok_or_else(|| FareError::new("routes.txt has no route_type column"))?;
    let mut linked = Vec::new();
    for row in rows {
        if row
            .get(type_column)
            .is_some_and(|route_type| route_type == "2")
        {
            if let Some(route_id) = row.get(id_column) {
                linked.push(vec![NETWORK_ID.to_string(), route_id.to_string()]);
            }
        }
    }
    linked.sort();
    linked.dedup();
    Ok(linked)
}

/// Parses a GTFS CSV table into owned rows.
///
/// # Arguments
///
/// * `bytes` — File bytes. A leading UTF-8 BOM is ignored.
/// * `name` — File name used in errors.
///
/// # Returns
///
/// The header and each data row.
///
/// # Errors
///
/// Returns [`FareError`] when the bytes are not CSV.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn read_table(bytes: &[u8], name: &str) -> Result<(Vec<String>, Vec<Vec<String>>), FareError> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    let header = reader
        .headers()
        .map_err(|err| FareError::new(format!("{name}: {err}")))?
        .iter()
        .map(ToOwned::to_owned)
        .collect();
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|err| FareError::new(format!("{name}: {err}")))?;
        rows.push(record.iter().map(ToOwned::to_owned).collect());
    }
    Ok((header, rows))
}

/// Encodes rows as CSV.
///
/// # Arguments
///
/// * `header` — Column names.
/// * `rows` — Data rows in the same order as `header`.
///
/// # Returns
///
/// UTF-8 CSV bytes without a BOM.
///
/// # Errors
///
/// Returns [`FareError`] when a row cannot be written.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn write_records(header: &[&str], rows: Vec<Vec<String>>) -> Result<Vec<u8>, FareError> {
    let mut writer = csv::WriterBuilder::new()
        .flexible(true)
        .from_writer(Vec::new());
    writer
        .write_record(header)
        .map_err(|err| FareError::new(err.to_string()))?;
    for row in rows {
        writer
            .write_record(&row)
            .map_err(|err| FareError::new(err.to_string()))?;
    }
    writer
        .flush()
        .map_err(|err| FareError::new(err.to_string()))?;
    writer
        .into_inner()
        .map_err(|err| FareError::new(err.to_string()))
}

/// Packs `files` into a deflated zip.
///
/// # Arguments
///
/// * `files` — File name to contents. Names are sorted because of `BTreeMap`.
///
/// # Returns
///
/// Zip bytes.
///
/// # Errors
///
/// Returns [`FareError`] when a zip entry cannot be written.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// See [`augment_rail_gtfs`].
///
/// ```
/// use njt_gtfs_proxy::augment_rail_gtfs;
///
/// assert!(augment_rail_gtfs(b"x").is_ok());
/// ```
fn write_zip(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, FareError> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in files {
        writer
            .start_file(name, options)
            .map_err(|err| FareError::new(err.to_string()))?;
        writer
            .write_all(bytes)
            .map_err(|err| FareError::new(err.to_string()))?;
    }
    let cursor = writer
        .finish()
        .map_err(|err| FareError::new(err.to_string()))?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
#[allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]
mod tests {
    use super::*;

    fn sample_zip() -> Vec<u8> {
        let mut files = BTreeMap::new();
        files.insert(
            "stops.txt".to_string(),
            b"stop_id,stop_name,zone_id\n105,NEW YORK PENN STATION,329\n148,TRENTON TRANSIT CENTER,5709\n63,HOBOKEN,336\n6900,MILITARY PARK LIGHT RAIL STATION,336\n".to_vec(),
        );
        files.insert(
            "routes.txt".to_string(),
            b"route_id,route_short_name,route_type\n10,NEC,2\n4,HBLR,0\n".to_vec(),
        );
        write_zip(&files).expect("sample zip")
    }

    fn read_named(zip_bytes: &[u8], name: &str) -> String {
        let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).expect("zip");
        let mut file = archive.by_name(name).expect(name);
        let mut text = String::new();
        file.read_to_string(&mut text).expect("read");
        text
    }

    #[test]
    fn non_zip_is_unchanged() {
        let augmented = augment_rail_gtfs(b"PK\x03\x04fake-zip").expect("passthrough");
        assert!(!augmented.fares_added);
        assert_eq!(augmented.bytes, b"PK\x03\x04fake-zip");
    }

    #[test]
    fn ny_to_trenton_is_zone_pair_and_light_rail_is_untouched() {
        let augmented = augment_rail_gtfs(&sample_zip()).expect("augment");
        assert!(augmented.fares_added);
        let stops = read_named(&augmented.bytes, "stops.txt");
        assert!(stops.contains("105,NEW YORK PENN STATION,njt_nyp"));
        assert!(stops.contains("148,TRENTON TRANSIT CENTER,njt_zone_22"));
        assert!(stops.contains("63,HOBOKEN,njt_hoboken"));
        assert!(stops.contains("6900,MILITARY PARK LIGHT RAIL STATION,336"));
        let rules = read_named(&augmented.bytes, "fare_rules.txt");
        assert!(rules.contains("ow_adult_njt_nyp_njt_zone_22,njt_nyp,njt_zone_22"));
        assert!(rules.contains("ow_adult_njt_nyp_njt_zone_22,njt_zone_22,njt_nyp"));
        assert!(!rules.contains("njt_nyp,njt_hoboken"));
        let attributes = read_named(&augmented.bytes, "fare_attributes.txt");
        assert!(attributes.contains("ow_adult_njt_nyp_njt_zone_22,20.35,USD,1,"));
        let areas = read_named(&augmented.bytes, "areas.txt");
        assert!(areas.contains("njt_zone_22,Rail zone 22"));
        let routes = read_named(&augmented.bytes, "route_networks.txt");
        assert!(routes.contains("njt_rail,10"));
        assert!(!routes.contains(",4"));
        let join = read_named(&augmented.bytes, "fare_leg_join_rules.txt");
        assert!(join.contains("njt_rail,njt_rail"));
        let leg = read_named(&augmented.bytes, "fare_leg_rules.txt");
        assert!(leg.contains("ow_senior_njt_nyp_njt_zone_22"));
        assert!(leg.contains("monthly_njt_nyp_njt_zone_22"));
        assert!(!leg.contains("timeframe"));
        let stop_areas = read_named(&augmented.bytes, "stop_areas.txt");
        assert!(!stop_areas.contains("6900,"));
    }

    #[test]
    fn suffern_exception_is_not_the_zone_price_and_airport_is_absent() {
        let catalog = catalog();
        let suffern = catalog
            .fares
            .iter()
            .find(|fare| {
                fare.origin_area_id == "njt_nyp"
                    && fare.destination_area_id == "njt_exc_mahwah_suffern"
            })
            .expect("suffern");
        assert_eq!(suffern.adult_ow, "14.80");
        assert_eq!(suffern.adult_ow, "14.80");
        assert_ne!(suffern.monthly, "477.00");
        assert!(catalog
            .stations
            .values()
            .all(|station| station.stop_id != "37953"));
        assert!(catalog.stations.contains_key("105"));
    }
}
