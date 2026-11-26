//! Platform stops inserted into the rail GTFS zip.
//!
//! The developer feed's `stops.txt` has empty `location_type` and
//! `parent_station` values and no `platform_code`. `stop_times.txt` references
//! the station `stop_id`, so that id stays a stop. A new parent
//! (`{stop_id}:station`, `location_type` 1) is added, the original stop becomes
//! its generic child, and each catalog track becomes `{stop_id}:track:{label}`.
//! That is the MBTA commuter-rail shape: the scheduled stop stays put, and
//! realtime assigns the track with `assigned_stop_id`. Stop ids follow the
//! Amtrak GTFS-RT proxy (`NYP:track:12`).

use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::catalog::{parent_stop_id, platform_stop_id, Station};
use super::index::StaticIndex;

/// Result of reading a zip and maybe rewriting `stops.txt`.
#[derive(Debug)]
pub struct Injection {
    /// Rewritten zip. `None` when no station changed and the input can be served as-is.
    pub zip: Option<Vec<u8>>,
    /// Trip and stop index from the input, before or after a rewrite. `trips.txt` is not edited.
    pub index: StaticIndex,
}

/// Adds platform stops for `stations` and indexes trips.
///
/// # Arguments
///
/// * `zip` — Upstream rail GTFS zip.
/// * `stations` — Catalog entries to publish. The order is the order rows are appended.
///
/// # Returns
///
/// The index, plus a new zip when at least one station gained a parent.
///
/// # Errors
///
/// Returns a message when the archive, `stops.txt`, or `trips.txt` cannot be read.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::{inject_platforms, station};
///
/// assert!(inject_platforms(b"not-a-zip", &[station("NY").unwrap()]).is_err());
/// ```
pub fn inject_platforms(zip: &[u8], stations: &[&Station]) -> Result<Injection, String> {
    let index = StaticIndex::from_zip(zip)?;
    let stops = read_entry(zip, "stops.txt")?;
    let rewritten = rewrite_stops(&stops, stations)?;
    if !rewritten.changed {
        return Ok(Injection { zip: None, index });
    }
    let archive = replace_stops(zip, &rewritten.bytes)?;
    Ok(Injection {
        zip: Some(archive),
        index,
    })
}

/// CSV rewrite of `stops.txt`.
struct RewrittenStops {
    /// New file bytes.
    bytes: Vec<u8>,
    /// Whether a parent was added.
    changed: bool,
}

/// Inserts parents and track stops.
///
/// # Arguments
///
/// * `raw` — Original `stops.txt`.
/// * `stations` — Stations to add.
///
/// # Returns
///
/// The rewritten CSV and whether it differs in meaning from the input.
///
/// # Errors
///
/// Returns a message when the header has no `stop_id` or the CSV is unreadable.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::inject_platforms;
///
/// assert!(inject_platforms(b"PK", &[]).is_err());
/// ```
fn rewrite_stops(raw: &[u8], stations: &[&Station]) -> Result<RewrittenStops, String> {
    let body = raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(raw);
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(body);
    let mut headers: Vec<String> = reader
        .headers()
        .map_err(|err| err.to_string())?
        .iter()
        .map(|header| header.trim().to_string())
        .collect();
    if !headers.iter().any(|header| header == "stop_id") {
        return Err("stops.txt has no stop_id".to_string());
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|err| err.to_string())?;
        rows.push(record.iter().map(str::to_string).collect());
    }
    for column in [
        "location_type",
        "parent_station",
        "platform_code",
        "platform_name",
        "level_id",
    ] {
        if !headers.iter().any(|header| header == column) {
            headers.push(column.to_string());
        }
    }
    for row in &mut rows {
        row.resize(headers.len(), String::new());
    }
    let column = |name: &str| {
        headers
            .iter()
            .position(|header| header == name)
            .expect("column added")
    };
    let id_col = column("stop_id");
    let code_col = column_optional(&headers, "stop_code");
    let name_col = column_optional(&headers, "stop_name");
    let location_col = column("location_type");
    let parent_col = column("parent_station");
    let platform_col = column("platform_code");
    let platform_name_col = column("platform_name");
    let existing: Vec<String> = rows.iter().map(|row| row[id_col].clone()).collect();
    let mut added = Vec::new();
    let mut changed = false;
    for station in stations {
        let Some(index) = find_station_row(&rows, id_col, code_col, name_col, station) else {
            tracing::warn!(station = station.code, "track station was not in stops.txt");
            continue;
        };
        let location = rows[index][location_col].trim();
        let parent = rows[index][parent_col].trim();
        if !matches!(location, "" | "0") || !parent.is_empty() {
            tracing::warn!(
                station = station.code,
                "track station is not a plain unparented stop; left unchanged"
            );
            continue;
        }
        let stop_id = rows[index][id_col].clone();
        let parent_id = parent_stop_id(&stop_id);
        if existing.iter().any(|id| id == &parent_id)
            || added
                .iter()
                .any(|row: &Vec<String>| row[id_col] == parent_id)
        {
            tracing::warn!(station = station.code, "platform parent id already exists");
            continue;
        }
        let template = rows[index].clone();
        rows[index][parent_col] = parent_id.clone();
        if rows[index][platform_name_col].trim().is_empty() {
            rows[index][platform_name_col] = "Rail".to_string();
        }
        added.push(station_row(&headers, &template, &parent_id));
        let name = name_col
            .map(|col| template[col].clone())
            .unwrap_or_default();
        for track in station.tracks {
            let mut row = copy_kept(
                &headers,
                &template,
                &[
                    "stop_name",
                    "stop_lat",
                    "stop_lon",
                    "zone_id",
                    "stop_timezone",
                    "wheelchair_boarding",
                    "level_id",
                ],
            );
            row[id_col] = platform_stop_id(&stop_id, track);
            row[location_col] = "0".to_string();
            row[parent_col] = parent_id.clone();
            row[platform_col] = (*track).to_string();
            row[platform_name_col] = format!("Rail - Track {track}");
            if let Some(desc) = column_optional(&headers, "stop_desc") {
                row[desc] = format!("{name} - Rail - Track {track}");
            }
            added.push(row);
        }
        changed = true;
    }
    if !changed {
        return Ok(RewrittenStops {
            bytes: Vec::new(),
            changed: false,
        });
    }
    rows.extend(added);
    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    writer
        .write_record(&headers)
        .map_err(|err| err.to_string())?;
    for row in &rows {
        writer.write_record(row).map_err(|err| err.to_string())?;
    }
    let bytes = writer.into_inner().map_err(|err| err.to_string())?;
    Ok(RewrittenStops {
        bytes,
        changed: true,
    })
}

/// Finds the stop row for `station`.
///
/// # Arguments
///
/// * `rows` — Data rows padded to the header width.
/// * `id_col` — `stop_id` column.
/// * `code_col` — `stop_code` column, when the file has one.
/// * `name_col` — `stop_name` column, when the file has one.
/// * `station` — Catalog entry.
///
/// # Returns
///
/// The row index. `stop_code` wins when it equals the station code and the name
/// matches. Otherwise the observed `stop_id` is used, again only when the name matches.
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
/// assert_eq!(station("HB").unwrap().stop_id, "63");
/// ```
fn find_station_row(
    rows: &[Vec<String>],
    id_col: usize,
    code_col: Option<usize>,
    name_col: Option<usize>,
    station: &Station,
) -> Option<usize> {
    let name_ok = |row: &Vec<String>| {
        name_col.is_none_or(|col| {
            row[col]
                .to_ascii_uppercase()
                .contains(station.name_fragment)
        })
    };
    if let Some(code_col) = code_col {
        if let Some(index) = rows
            .iter()
            .position(|row| row[code_col].trim().eq_ignore_ascii_case(station.code) && name_ok(row))
        {
            return Some(index);
        }
    }
    rows.iter()
        .position(|row| row[id_col].trim() == station.stop_id && name_ok(row))
}

/// Builds the `location_type` 1 parent from the original stop.
///
/// # Arguments
///
/// * `headers` — Output header.
/// * `template` — The existing station row.
/// * `parent_id` — New stop id.
///
/// # Returns
///
/// A parent row. `stop_code` is cleared so it is not duplicated.
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
/// assert!(parent_stop_id("105").ends_with(":station"));
/// ```
fn station_row(headers: &[String], template: &[String], parent_id: &str) -> Vec<String> {
    let mut row = copy_kept(
        headers,
        template,
        &[
            "stop_name",
            "stop_desc",
            "stop_lat",
            "stop_lon",
            "zone_id",
            "stop_timezone",
            "stop_url",
            "wheelchair_boarding",
            "level_id",
        ],
    );
    let id_col = headers
        .iter()
        .position(|header| header == "stop_id")
        .expect("stop_id");
    let location_col = headers
        .iter()
        .position(|header| header == "location_type")
        .expect("location");
    row[id_col] = parent_id.to_string();
    row[location_col] = "1".to_string();
    row
}

/// Copies selected columns into a blank row of `headers`' width.
///
/// # Arguments
///
/// * `headers` — Column names.
/// * `template` — Source row, aligned with `headers`.
/// * `keep` — Column names to copy.
///
/// # Returns
///
/// A row with every other column empty.
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
/// assert_eq!(platform_stop_id("63", "1"), "63:track:1");
/// ```
fn copy_kept(headers: &[String], template: &[String], keep: &[&str]) -> Vec<String> {
    headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            if keep.iter().any(|name| name == header) {
                template.get(index).cloned().unwrap_or_default()
            } else {
                String::new()
            }
        })
        .collect()
}

/// Returns the index of `name`, if the header has it.
///
/// # Arguments
///
/// * `headers` — Column names.
/// * `name` — Column to find.
///
/// # Returns
///
/// The index, or `None`.
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
/// assert!(default_codes().contains(&"NY"));
/// ```
fn column_optional(headers: &[String], name: &str) -> Option<usize> {
    headers.iter().position(|header| header == name)
}

/// Replaces `stops.txt` and copies every other entry without recompressing it.
///
/// # Arguments
///
/// * `zip` — Original archive.
/// * `stops` — Replacement `stops.txt` bytes.
///
/// # Returns
///
/// A new zip.
///
/// # Errors
///
/// Returns a message when an entry cannot be copied.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::inject_platforms;
///
/// assert!(inject_platforms(&[], &[]).is_err());
/// ```
fn replace_stops(zip: &[u8], stops: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive = ZipArchive::new(Cursor::new(zip)).map_err(|err| err.to_string())?;
    let names: Vec<String> = (0..archive.len())
        .filter_map(|index| archive.name_for_index(index).map(str::to_string))
        .collect();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut replaced = false;
    for name in &names {
        if is_stops(name) {
            writer
                .start_file(name, options)
                .map_err(|err| err.to_string())?;
            writer.write_all(stops).map_err(|err| err.to_string())?;
            replaced = true;
            continue;
        }
        let file = archive.by_name(name).map_err(|err| err.to_string())?;
        writer.raw_copy_file(file).map_err(|err| err.to_string())?;
    }
    if !replaced {
        return Err("stops.txt was not in the archive".to_string());
    }
    writer
        .finish()
        .map_err(|err| err.to_string())
        .map(|cursor| cursor.into_inner())
}

/// Reads one archive entry.
///
/// # Arguments
///
/// * `zip` — Archive bytes.
/// * `name` — Root file name.
///
/// # Returns
///
/// The entry bytes.
///
/// # Errors
///
/// Returns a message when the entry is missing.
///
/// # Panics
///
/// This function does not panic.
///
/// # Examples
///
/// ```
/// use njt_gtfs_proxy::tracks::inject_platforms;
///
/// assert!(inject_platforms(b"nope", &[]).is_err());
/// ```
fn read_entry(zip: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let mut archive = ZipArchive::new(Cursor::new(zip)).map_err(|err| err.to_string())?;
    let mut file = archive
        .by_name(name)
        .map_err(|_| format!("missing {name}"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    Ok(bytes)
}

/// Reports whether `name` is `stops.txt`.
///
/// # Arguments
///
/// * `name` — Zip entry name.
///
/// # Returns
///
/// `true` for `stops.txt` at the root or one directory down.
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
/// use njt_gtfs_proxy::tracks::STATIONS;
///
/// assert_eq!(STATIONS.len(), 10);
/// ```
fn is_stops(name: &str) -> bool {
    name == "stops.txt" || name.ends_with("/stops.txt")
}
