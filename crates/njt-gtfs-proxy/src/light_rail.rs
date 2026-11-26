//! Flat fares for NJ Transit light rail.
//!
//! Hudson-Bergen Light Rail, Newark Light Rail, and the River LINE each have
//! one fare for any trip on that system. The amounts are the charts NJ Transit
//! links from its adjusted-fares page (PDF titles dated 22 Apr 2026, in effect
//! with the 1 Jul 2026 fare change). A trip-planner check on 2 Dec 2026
//! returned the same one-way adult and reduced prices.
//!
//! # Examples
//!
//! ```
//! use njt_gtfs_proxy::light_rail::NETWORK_ID;
//!
//! assert_eq!(NETWORK_ID, "njt_light_rail");
//! ```

use std::collections::{HashMap, HashSet};

use crate::fares::FareError;

/// Parses a GTFS CSV table.
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
/// See [`contribute`].
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

/// Network id for every light rail route.
pub const NETWORK_ID: &str = "njt_light_rail";

/// Rider-facing network name.
pub const NETWORK_NAME: &str = "NJ TRANSIT Light Rail";

/// Checked-in light rail fare rows.
const LIGHT_RAIL_CSV: &str = include_str!("../../../data/fares/light_rail.csv");

/// Stop ids that belong to each system in the 19 Nov 2026 rail GTFS.
///
/// The developer zip lists these stops but does not put them on any trip, so
/// trip matching alone would leave them unpriced.
const LIGHT_RAIL_STOPS_CSV: &str = include_str!("../../../data/fares/light_rail_stops.csv");

/// One system, or the Newark underground discount.
struct System {
    /// Area id. Also the Fares v1 zone for a full-system row.
    area_id: String,
    /// Area name.
    area_name: String,
    /// Leg group. The underground discount uses the Newark Light Rail group.
    leg_group: String,
    /// `routes.route_short_name` values that belong to this system.
    short_names: Vec<String>,
    /// Adult one-way amount.
    adult_ow: String,
    /// Child, senior, and disabled one-way amount.
    reduced_ow: String,
    /// Adult 10-trip amount. Empty when the chart has no such column.
    ten_trip: String,
    /// FlexPass amount.
    flexpass: String,
    /// Adult monthly amount.
    monthly: String,
    /// School monthly amount.
    student_monthly: String,
    /// Ticket lifetime in seconds. Empty for the underground discount.
    transfer_seconds: String,
    /// Newark underground stations only.
    underground: bool,
}

/// Light rail rows to merge into the fare files.
#[derive(Default)]
pub(crate) struct Contribution {
    /// `stop_id` to the system area written into `stops.zone_id`.
    pub zone_by_stop: HashMap<String, String>,
    /// Extra `stop_id, area_id` rows, including the underground area.
    pub stop_areas: Vec<(String, String)>,
    /// Light rail areas.
    pub areas: Vec<(String, String)>,
    /// `network_id, route_id` rows.
    pub routes: Vec<Vec<String>>,
    /// `fare_products.txt` data rows.
    pub products: Vec<Vec<String>>,
    /// `fare_leg_rules.txt` data rows.
    pub leg_rules: Vec<Vec<String>>,
    /// Fares v1 `fare_attributes.txt` data rows.
    pub attributes: Vec<Vec<String>>,
    /// Fares v1 `fare_rules.txt` data rows.
    pub fare_rules: Vec<Vec<String>>,
    /// `fare_transfer_rules.txt` data rows.
    pub transfers: Vec<Vec<String>>,
}

/// Builds the light rail fare rows present in this zip.
///
/// # Arguments
///
/// * `stop_rows` — `stop_id` and `stop_name` from `stops.txt`.
/// * `routes_txt` — Upstream `routes.txt`, when the zip has one.
/// * `trips_txt` — Upstream `trips.txt`, when the zip has one.
/// * `stop_times_txt` — Upstream `stop_times.txt`, when the zip has one.
///
/// # Returns
///
/// Areas, products, and the stops that belong to each system. Empty when the
/// zip has no light rail route.
///
/// # Errors
///
/// Returns [`FareError`] when a GTFS table or the checked-in fare CSV is not
/// readable.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::light_rail::is_underground;
///
/// assert!(is_underground("PENN STATION LIGHT RAIL DEPARTURE"));
/// ```
pub(crate) fn contribute(
    stop_rows: &[(String, String)],
    routes_txt: Option<&[u8]>,
    trips_txt: Option<&[u8]>,
    stop_times_txt: Option<&[u8]>,
) -> Result<Contribution, FareError> {
    let systems = read_systems()?;
    let Some(routes_txt) = routes_txt else {
        return Ok(Contribution::default());
    };
    let route_ids = light_rail_route_ids(routes_txt, &systems)?;
    let names: HashMap<String, String> = stop_rows.iter().cloned().collect();
    let listed = catalog_stops()?;
    let stops_by_short = stops_for_systems(trips_txt, stop_times_txt, &route_ids)?;
    let mut zone_by_stop = HashMap::new();
    let mut stop_areas = Vec::new();
    let mut used_areas = HashSet::new();
    for system in systems.iter().filter(|system| !system.underground) {
        let mut stops = HashSet::new();
        for short_name in &system.short_names {
            if let Some(found) = stops_by_short.get(short_name) {
                stops.extend(found.iter().cloned());
            }
        }
        for (stop_id, short_name) in &listed {
            if system.short_names.iter().any(|name| name == short_name)
                && names.contains_key(stop_id)
            {
                stops.insert(stop_id.clone());
            }
        }
        if stops.is_empty() {
            continue;
        }
        used_areas.insert(system.area_id.clone());
        for stop_id in stops {
            zone_by_stop.insert(stop_id.clone(), system.area_id.clone());
            stop_areas.push((stop_id, system.area_id.clone()));
        }
    }
    let underground = systems.iter().find(|system| system.underground);
    if let Some(underground) = underground {
        let nlr_stops: Vec<String> = zone_by_stop
            .iter()
            .filter(|(_, area)| area.as_str() == "njt_nlr")
            .map(|(stop_id, _)| stop_id.clone())
            .collect();
        let mut any = false;
        for stop_id in nlr_stops {
            let name = names.get(&stop_id).map(String::as_str).unwrap_or("");
            if is_underground(name) {
                stop_areas.push((stop_id, underground.area_id.clone()));
                any = true;
            }
        }
        if any {
            used_areas.insert(underground.area_id.clone());
        }
    }

    let mut areas = Vec::new();
    let mut products = Vec::new();
    let mut leg_rules = Vec::new();
    let mut attributes = Vec::new();
    let mut fare_rules = Vec::new();
    let mut transfers = Vec::new();
    for system in &systems {
        if !used_areas.contains(&system.area_id) {
            continue;
        }
        areas.push((system.area_id.clone(), system.area_name.clone()));
        push_products(system, &mut products, &mut leg_rules);
        if !system.underground {
            attributes.push(vec![
                format!("ow_adult_{}", system.area_id),
                system.adult_ow.clone(),
                "USD".to_string(),
                "1".to_string(),
                String::new(),
                "NJT".to_string(),
                system.transfer_seconds.clone(),
            ]);
            fare_rules.push(vec![
                format!("ow_adult_{}", system.area_id),
                system.area_id.clone(),
                system.area_id.clone(),
            ]);
            if !system.transfer_seconds.is_empty() {
                transfers.push(vec![
                    system.leg_group.clone(),
                    system.leg_group.clone(),
                    "3".to_string(),
                    system.transfer_seconds.clone(),
                    "1".to_string(),
                    "0".to_string(),
                    String::new(),
                ]);
            }
        }
    }
    let mut routes = Vec::new();
    for (short_name, ids) in &route_ids {
        if systems.iter().any(|system| {
            !system.underground
                && system.short_names.iter().any(|name| name == short_name)
                && used_areas.contains(&system.area_id)
        }) {
            for route_id in ids {
                routes.push(vec![NETWORK_ID.to_string(), route_id.clone()]);
            }
        }
    }
    routes.sort();
    routes.dedup();
    areas.sort();
    stop_areas.sort();
    products.sort();
    leg_rules.sort();
    leg_rules.dedup();
    Ok(Contribution {
        zone_by_stop,
        stop_areas,
        areas,
        routes,
        products,
        leg_rules,
        attributes,
        fare_rules,
        transfers,
    })
}

/// Stop id to `HBLR`, `NLR`, or `RVLN` from the checked-in stop list.
///
/// # Errors
///
/// Returns [`FareError`] when the stop CSV cannot be read.
///
/// # Panics
///
/// This function does not panic.
fn catalog_stops() -> Result<Vec<(String, String)>, FareError> {
    let mut reader = csv::Reader::from_reader(LIGHT_RAIL_STOPS_CSV.as_bytes());
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|err| FareError::new(err.to_string()))?;
        let stop_id = record
            .get(0)
            .ok_or_else(|| FareError::new("light_rail_stops.csv has no stop_id"))?;
        let system = record
            .get(2)
            .ok_or_else(|| FareError::new("light_rail_stops.csv has no system"))?;
        let short_name = match system {
            "hblr" => "HBLR",
            "nlr" => "NLR",
            "river" => "RVLN",
            other => {
                return Err(FareError::new(format!(
                    "light_rail_stops.csv has unknown system {other}"
                )))
            }
        };
        rows.push((stop_id.to_string(), short_name.to_string()));
    }
    Ok(rows)
}

/// Reads the checked-in light rail chart.
///
/// # Errors
///
/// Returns [`FareError`] when the CSV is missing a required column.
///
/// # Panics
///
/// This function does not panic.
fn read_systems() -> Result<Vec<System>, FareError> {
    let mut reader = csv::Reader::from_reader(LIGHT_RAIL_CSV.as_bytes());
    let headers = reader
        .headers()
        .map_err(|err| FareError::new(err.to_string()))?
        .clone();
    let mut systems = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|err| FareError::new(err.to_string()))?;
        let field = |name: &str| -> Result<String, FareError> {
            headers
                .iter()
                .position(|header| header == name)
                .and_then(|index| record.get(index))
                .map(ToOwned::to_owned)
                .ok_or_else(|| FareError::new(format!("light_rail.csv has no {name} column")))
        };
        systems.push(System {
            area_id: field("area_id")?,
            area_name: field("area_name")?,
            leg_group: field("leg_group_id")?,
            short_names: field("route_short_names")?
                .split_whitespace()
                .map(ToOwned::to_owned)
                .collect(),
            adult_ow: field("adult_ow")?,
            reduced_ow: field("reduced_ow")?,
            ten_trip: field("ten_trip")?,
            flexpass: field("flexpass")?,
            monthly: field("monthly")?,
            student_monthly: field("student_monthly")?,
            transfer_seconds: field("transfer_seconds")?,
            underground: field("underground")? == "1",
        });
    }
    Ok(systems)
}

/// Maps each light rail short name to the route ids that use it.
///
/// # Errors
///
/// Returns [`FareError`] when `routes.txt` is not CSV.
fn light_rail_route_ids(
    routes_txt: &[u8],
    systems: &[System],
) -> Result<HashMap<String, Vec<String>>, FareError> {
    let (header, rows) = read_table(routes_txt, "routes.txt")?;
    let id_column = header
        .iter()
        .position(|name| name == "route_id")
        .ok_or_else(|| FareError::new("routes.txt has no route_id column"))?;
    let short_column = header
        .iter()
        .position(|name| name == "route_short_name")
        .ok_or_else(|| FareError::new("routes.txt has no route_short_name column"))?;
    let wanted: HashSet<&str> = systems
        .iter()
        .filter(|system| !system.underground)
        .flat_map(|system| system.short_names.iter().map(String::as_str))
        .collect();
    let mut ids: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let Some(short_name) = row.get(short_column) else {
            continue;
        };
        if !wanted.contains(short_name.as_str()) {
            continue;
        }
        if let Some(route_id) = row.get(id_column) {
            ids.entry(short_name.clone())
                .or_default()
                .push(route_id.clone());
        }
    }
    Ok(ids)
}

/// Stop ids visited by each light rail short name.
///
/// # Errors
///
/// Returns [`FareError`] when `trips.txt` or `stop_times.txt` is not CSV.
fn stops_for_systems(
    trips_txt: Option<&[u8]>,
    stop_times_txt: Option<&[u8]>,
    route_ids: &HashMap<String, Vec<String>>,
) -> Result<HashMap<String, HashSet<String>>, FareError> {
    let (Some(trips_txt), Some(stop_times_txt)) = (trips_txt, stop_times_txt) else {
        return Ok(HashMap::new());
    };
    let route_of: HashMap<&str, &str> = route_ids
        .iter()
        .flat_map(|(short_name, ids)| ids.iter().map(move |id| (id.as_str(), short_name.as_str())))
        .collect();
    let (trip_header, trip_rows) = read_table(trips_txt, "trips.txt")?;
    let trip_id_column = trip_header
        .iter()
        .position(|name| name == "trip_id")
        .ok_or_else(|| FareError::new("trips.txt has no trip_id column"))?;
    let trip_route_column = trip_header
        .iter()
        .position(|name| name == "route_id")
        .ok_or_else(|| FareError::new("trips.txt has no route_id column"))?;
    let mut short_of_trip = HashMap::new();
    for row in trip_rows {
        let (Some(trip_id), Some(route_id)) = (row.get(trip_id_column), row.get(trip_route_column))
        else {
            continue;
        };
        if let Some(short_name) = route_of.get(route_id.as_str()) {
            short_of_trip.insert(trip_id.clone(), (*short_name).to_string());
        }
    }
    let (time_header, time_rows) = read_table(stop_times_txt, "stop_times.txt")?;
    let time_trip_column = time_header
        .iter()
        .position(|name| name == "trip_id")
        .ok_or_else(|| FareError::new("stop_times.txt has no trip_id column"))?;
    let time_stop_column = time_header
        .iter()
        .position(|name| name == "stop_id")
        .ok_or_else(|| FareError::new("stop_times.txt has no stop_id column"))?;
    let mut stops: HashMap<String, HashSet<String>> = HashMap::new();
    for row in time_rows {
        let (Some(trip_id), Some(stop_id)) = (row.get(time_trip_column), row.get(time_stop_column))
        else {
            continue;
        };
        if let Some(short_name) = short_of_trip.get(trip_id) {
            stops
                .entry(short_name.clone())
                .or_default()
                .insert(stop_id.clone());
        }
    }
    Ok(stops)
}

/// Whether `name` is one of the four Newark Light Rail underground stations.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::light_rail::is_underground;
///
/// assert!(is_underground("MILITARY PARK LIGHT RAIL STATION"));
/// assert!(!is_underground("NEWARK PENN STATION"));
/// ```
pub fn is_underground(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.contains("MILITARY PARK")
        || name.contains("WARREN STREET")
        || name.contains("WASHINGTON STREET")
        || (name.contains("PENN STATION") && name.contains("LIGHT RAIL"))
}

/// Adds the non-empty products for one system.
fn push_products(
    system: &System,
    products: &mut Vec<Vec<String>>,
    leg_rules: &mut Vec<Vec<String>>,
) {
    let mut kinds = vec![
        (
            format!("ow_adult_{}", system.area_id),
            format!("Adult one-way on {}", system.area_name),
            "adult",
            "njt_ticket",
            system.adult_ow.clone(),
        ),
        (
            format!("ow_child_{}", system.area_id),
            format!("Child one-way on {}", system.area_name),
            "child",
            "njt_ticket",
            system.reduced_ow.clone(),
        ),
        (
            format!("ow_senior_{}", system.area_id),
            format!("Senior one-way on {}", system.area_name),
            "senior",
            "njt_ticket",
            system.reduced_ow.clone(),
        ),
        (
            format!("ow_disabled_{}", system.area_id),
            format!("Disabled one-way on {}", system.area_name),
            "disabled",
            "njt_ticket",
            system.reduced_ow.clone(),
        ),
        (
            format!("ten_trip_{}", system.area_id),
            format!("10-trip on {}", system.area_name),
            "adult",
            "njt_ten_trip",
            system.ten_trip.clone(),
        ),
        (
            format!("flexpass_{}", system.area_id),
            format!("FlexPass on {}", system.area_name),
            "adult",
            "njt_flexpass",
            system.flexpass.clone(),
        ),
        (
            format!("monthly_{}", system.area_id),
            format!("Monthly pass on {}", system.area_name),
            "adult",
            "njt_monthly",
            system.monthly.clone(),
        ),
        (
            format!("school_monthly_{}", system.area_id),
            format!("School monthly on {}", system.area_name),
            "student",
            "njt_school_monthly",
            system.student_monthly.clone(),
        ),
    ];
    kinds.retain(|kind| !kind.4.is_empty());
    for (id, name, rider, media, amount) in kinds {
        products.push(vec![
            id.clone(),
            name,
            rider.to_string(),
            media.to_string(),
            amount,
            "USD".to_string(),
        ]);
        leg_rules.push(vec![
            system.leg_group.clone(),
            NETWORK_ID.to_string(),
            system.area_id.clone(),
            system.area_id.clone(),
            id,
        ]);
    }
}
