//! In-memory track state: static index, optional station boards, and polling.
//!
//! Headsign parsing needs no extra RailData calls. Station boards are fetched
//! only when the operator turns that fallback on.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use njt_client::{Client, Clock};

use super::board::{parse_board, BoardRow};
use super::catalog::Station;
use super::enrich::{enrich_trip_updates, EnrichInput};
use super::index::StaticIndex;
use super::platforms::inject_platforms;

/// Shared track enrichment for the rail routes.
pub struct TrackHub {
    /// When false, zip and trip updates are served unchanged.
    enabled: bool,
    /// When true, poll TrainData station boards.
    boards_enabled: bool,
    /// Stations in poll and injection order.
    stations: Vec<&'static Station>,
    /// Time between the start of successive full board cycles.
    poll_every: Duration,
    /// Board rows older than this are ignored.
    board_ttl: Duration,
    /// TrainData client. `None` when boards are off or rail is off.
    client: Option<Client>,
    /// Clock for board freshness.
    clock: Arc<dyn Clock>,
    /// Static index from the last readable zip.
    index: RwLock<StaticIndex>,
    /// Set after a zip has been examined, including a zip that could not be parsed.
    index_ready: AtomicBool,
    /// Fresh board rows by station code.
    boards: RwLock<HashMap<String, CachedBoard>>,
}

/// One station's parsed board and the time it was stored.
struct CachedBoard {
    /// Parsed rows.
    rows: Vec<BoardRow>,
    /// When the body was accepted.
    fetched_at: DateTime<Utc>,
}

impl TrackHub {
    /// Builds a hub. No network call is made.
    ///
    /// # Arguments
    ///
    /// * `enabled` — Inject platforms and enrich trip updates.
    /// * `boards_enabled` — Poll station boards. Ignored when `enabled` is false.
    /// * `stations` — Catalog subset.
    /// * `poll_every` — Full-cycle period used when boards are on.
    /// * `client` — TrainData client. Required for polling.
    /// * `clock` — Freshness clock.
    ///
    /// # Returns
    ///
    /// An empty hub.
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
    /// use njt_gtfs_proxy::tracks::TrackHub;
    ///
    /// let _hub = TrackHub::new(false, false, Vec::new(), std::time::Duration::from_secs(600), None, std::sync::Arc::new(njt_client::SystemClock));
    /// ```
    pub fn new(
        enabled: bool,
        boards_enabled: bool,
        stations: Vec<&'static Station>,
        poll_every: Duration,
        client: Option<Client>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let board_ttl = poll_every.saturating_mul(2).max(Duration::from_secs(60));
        Self {
            enabled,
            boards_enabled: enabled && boards_enabled && client.is_some(),
            stations,
            poll_every,
            board_ttl,
            client,
            clock,
            index: RwLock::new(StaticIndex::default()),
            index_ready: AtomicBool::new(false),
            boards: RwLock::new(HashMap::new()),
        }
    }

    /// Reports whether rail bytes are rewritten.
    ///
    /// # Arguments
    ///
    /// This method takes no arguments.
    ///
    /// # Returns
    ///
    /// The `enabled` flag from [`TrackHub::new`].
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
    /// use njt_gtfs_proxy::tracks::TrackHub;
    /// use std::sync::Arc;
    ///
    /// let hub = TrackHub::new(true, false, Vec::new(), std::time::Duration::from_secs(600), None, Arc::new(njt_client::SystemClock));
    /// assert!(hub.enabled());
    /// assert!(!hub.boards_enabled());
    /// ```
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Reports whether station boards are polled.
    ///
    /// # Arguments
    ///
    /// This method takes no arguments.
    ///
    /// # Returns
    ///
    /// `true` only when enrichment and a TrainData client are both configured.
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
    /// use njt_gtfs_proxy::tracks::TrackHub;
    /// use std::sync::Arc;
    ///
    /// let hub = TrackHub::new(true, false, Vec::new(), std::time::Duration::from_secs(600), None, Arc::new(njt_client::SystemClock));
    /// assert!(!hub.boards_enabled());
    /// ```
    pub fn boards_enabled(&self) -> bool {
        self.boards_enabled
    }

    /// Reports whether a static zip has already been examined.
    ///
    /// # Arguments
    ///
    /// This method takes no arguments.
    ///
    /// # Returns
    ///
    /// `true` after [`TrackHub::inject`] has run.
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
    /// use njt_gtfs_proxy::tracks::TrackHub;
    /// use std::sync::Arc;
    ///
    /// let hub = TrackHub::new(false, false, Vec::new(), std::time::Duration::from_secs(600), None, Arc::new(njt_client::SystemClock));
    /// assert!(!hub.index_ready());
    /// ```
    pub fn index_ready(&self) -> bool {
        self.index_ready.load(Ordering::Relaxed)
    }

    /// Adds platform stops to a rail zip and remembers its trips.
    ///
    /// # Arguments
    ///
    /// * `raw` — Upstream zip. A body that is not a zip is returned unchanged.
    ///
    /// # Returns
    ///
    /// The injected zip, or `raw` when injection is off or the body cannot be rewritten.
    ///
    /// # Errors
    ///
    /// This method does not return an error.
    ///
    /// # Panics
    ///
    /// This method does not panic. A poisoned lock is recovered.
    ///
    /// # Examples
    ///
    /// ```
    /// use njt_gtfs_proxy::tracks::TrackHub;
    /// use std::sync::Arc;
    ///
    /// let hub = TrackHub::new(true, false, Vec::new(), std::time::Duration::from_secs(600), None, Arc::new(njt_client::SystemClock));
    /// let raw = bytes::Bytes::from_static(b"PK\x03\x04");
    /// assert_eq!(hub.inject(raw.clone()), raw);
    /// ```
    pub fn inject(&self, raw: Bytes) -> Bytes {
        if !self.enabled {
            return raw;
        }
        match inject_platforms(&raw, &self.stations) {
            Ok(injection) => {
                let mut guard = self.index.write().unwrap_or_else(|err| err.into_inner());
                *guard = injection.index;
                self.index_ready.store(true, Ordering::Relaxed);
                injection.zip.map(Bytes::from).unwrap_or(raw)
            }
            Err(err) => {
                tracing::debug!(error = %err, "rail GTFS was not rewritten with platform stops");
                self.index_ready.store(true, Ordering::Relaxed);
                raw
            }
        }
    }

    /// Applies headsign tracks, optional board tracks, and empty trip ids.
    ///
    /// # Arguments
    ///
    /// * `raw` — Upstream trip-update protobuf.
    ///
    /// # Returns
    ///
    /// The enriched protobuf, or `raw` when nothing changed or the body is not a feed.
    ///
    /// # Errors
    ///
    /// This method does not return an error.
    ///
    /// # Panics
    ///
    /// This method does not panic. A poisoned lock is recovered.
    ///
    /// # Examples
    ///
    /// ```
    /// use njt_gtfs_proxy::tracks::TrackHub;
    /// use std::sync::Arc;
    ///
    /// let hub = TrackHub::new(false, false, Vec::new(), std::time::Duration::from_secs(600), None, Arc::new(njt_client::SystemClock));
    /// let raw = bytes::Bytes::from_static(b"\x0afake");
    /// assert_eq!(hub.enrich(raw.clone()), raw);
    /// ```
    pub fn enrich(&self, raw: Bytes) -> Bytes {
        if !self.enabled {
            return raw;
        }
        let index = self.index.read().unwrap_or_else(|err| err.into_inner());
        let boards = self.fresh_boards();
        let input = EnrichInput {
            stations: &self.stations,
            index: &index,
            boards: &boards,
        };
        match enrich_trip_updates(&raw, &input) {
            Some(bytes) => Bytes::from(bytes),
            None => raw,
        }
    }

    /// Polls each station once per cycle until the task is aborted.
    ///
    /// # Arguments
    ///
    /// This method takes no arguments. The caller aborts the task on shutdown.
    ///
    /// # Returns
    ///
    /// Nothing. The method runs until the task is dropped.
    ///
    /// # Errors
    ///
    /// This method does not return an error. A failed board is logged and the
    /// next station is tried. The token is not logged.
    ///
    /// # Panics
    ///
    /// This method does not panic.
    ///
    /// # Examples
    ///
    /// ```
    /// use njt_gtfs_proxy::tracks::TrackHub;
    /// use std::sync::Arc;
    ///
    /// let hub = Arc::new(TrackHub::new(true, false, Vec::new(), std::time::Duration::from_secs(600), None, Arc::new(njt_client::SystemClock)));
    /// assert!(hub.spawn().is_none());
    /// ```
    pub async fn poll_forever(self: Arc<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let gap = self
            .poll_every
            .checked_div(self.stations.len().max(1) as u32)
            .unwrap_or(self.poll_every);
        loop {
            for station in &self.stations {
                match client.fetch_station_board(station.code).await {
                    Ok(body) => {
                        let rows = parse_board(&body, station);
                        let mut boards = self.boards.write().unwrap_or_else(|err| err.into_inner());
                        boards.insert(
                            station.code.to_string(),
                            CachedBoard {
                                rows,
                                fetched_at: self.clock.now(),
                            },
                        );
                    }
                    Err(err) => {
                        tracing::warn!(
                            station = station.code,
                            error = %err,
                            "station board was not updated"
                        );
                    }
                }
                tokio::time::sleep(gap).await;
            }
        }
    }

    /// Starts [`TrackHub::poll_forever`] when boards are enabled.
    ///
    /// # Arguments
    ///
    /// This method takes no arguments.
    ///
    /// # Returns
    ///
    /// The task, or `None` when boards are off.
    ///
    /// # Errors
    ///
    /// This method does not return an error.
    ///
    /// # Panics
    ///
    /// This method does not panic. It must be called inside a Tokio runtime.
    ///
    /// # Examples
    ///
    /// ```
    /// use njt_gtfs_proxy::tracks::TrackHub;
    /// use std::sync::Arc;
    ///
    /// let hub = Arc::new(TrackHub::new(false, false, Vec::new(), std::time::Duration::from_secs(600), None, Arc::new(njt_client::SystemClock)));
    /// assert!(hub.spawn().is_none());
    /// ```
    pub fn spawn(self: &Arc<Self>) -> Option<tokio::task::JoinHandle<()>> {
        if !self.boards_enabled {
            return None;
        }
        let hub = Arc::clone(self);
        Some(tokio::spawn(async move { hub.poll_forever().await }))
    }

    /// Copies board rows that are still inside the freshness window.
    ///
    /// # Arguments
    ///
    /// This method takes no arguments.
    ///
    /// # Returns
    ///
    /// Station code to rows. Stale stations are omitted.
    ///
    /// # Errors
    ///
    /// This method does not return an error.
    ///
    /// # Panics
    ///
    /// This method does not panic. A poisoned lock is recovered.
    ///
    /// # Examples
    ///
    /// Board freshness is covered by the enricher tests. A hub with boards off
    /// has nothing cached:
    ///
    /// ```
    /// use njt_gtfs_proxy::tracks::TrackHub;
    /// use std::sync::Arc;
    ///
    /// let hub = TrackHub::new(true, false, Vec::new(), std::time::Duration::from_secs(600), None, Arc::new(njt_client::SystemClock));
    /// assert!(!hub.boards_enabled());
    /// ```
    fn fresh_boards(&self) -> HashMap<String, Vec<BoardRow>> {
        let now = self.clock.now();
        let boards = self.boards.read().unwrap_or_else(|err| err.into_inner());
        boards
            .iter()
            .filter(|(_, board)| board_is_fresh(board.fetched_at, now, self.board_ttl))
            .map(|(code, board)| (code.clone(), board.rows.clone()))
            .collect()
    }
}

/// Reports whether a board stored at `fetched_at` is still inside `ttl`.
///
/// # Arguments
///
/// * `fetched_at` — When the board was stored.
/// * `now` — Current instant.
/// * `ttl` — Freshness window.
///
/// # Returns
///
/// `true` when `now` is strictly before `fetched_at + ttl`.
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
/// assert_eq!(board_calls_per_day(0, 600), 0);
/// ```
fn board_is_fresh(fetched_at: DateTime<Utc>, now: DateTime<Utc>, ttl: Duration) -> bool {
    let Ok(step) = chrono::Duration::from_std(ttl) else {
        return false;
    };
    fetched_at
        .checked_add_signed(step)
        .is_some_and(|expiry| now < expiry)
}
