# njt-gtfs-proxy

Local proxy that turns NJ Transit's token-authenticated GTFS APIs into plain GET URLs.

GTFS clients only know how to GET a static zip and GTFS-Realtime protobufs. NJ Transit's rail and bus developer APIs instead require `POST` with `multipart/form-data` and a short-lived token. This process holds your credentials, caches the token, and serves the feeds.

NJ Transit's data terms apply. Register at [developer.njtransit.com](https://developer.njtransit.com/registration) and use the username and password from the email they send you. Do not commit those credentials, tokens, or downloaded feeds.

## What the proxy adds

A plain passthrough would only hide the POST and the token. This process also keeps the token, caches responses, and, for rail, publishes track assignments in the shape the MBTA uses for commuter-rail platforms.

### Token, cache, and access key

Rail `getToken` and TrainData `getToken` each have their own 10-calls-per-America/New_York-day budget, shared with `isValidToken` on that API. This proxy never calls `isValidToken`. It mints lazily, stores the token in `NJT_TOKEN_CACHE` (mode `0600` on Unix) when that path is set, and stops at the local cap of 10. Rail and TrainData tokens are reused until the next Eastern midnight. Bus tokens are reused for 24 hours. Realtime responses are cached for `NJT_RT_TTL_SECS` (default 20). The static zip is cached for `NJT_STATIC_TTL_SECS` (default 6 hours). Set `NJT_PROXY_ACCESS_KEY` and send it as `x-njt-proxy-key` when the listen address is not only loopback.

### Rail GTFS without a token

Rail `getGTFS` answers an unauthenticated GET on both `testraildata.njtransit.com` and `raildata.njtransit.com`. The proxy tries that GET first and posts the token only when the GET is not a zip. Bus `getGTFS` stays a token POST. The developer rail zip has no platform columns filled in (`location_type` and `parent_station` are empty, and there is no `platform_code`). Every route in that zip, including light rail, is `route_type` 113. This proxy does not rewrite route types.

### Fares in `/rail/gtfs.zip`

The static rail zip gains Fares v2 files and legacy `fare_attributes.txt` / `fare_rules.txt`. Prices come from the njtransit.com trip planner. A 2026 PDF amount is used only when the planner has no price for that pair or that field. Each priced stop is its own fare area. Bus zips are not affected. Details are in [Rail fares](#rail-fares).

### Tracks in `/rail/trip-updates.pb`

NJ Transit's trip updates already carry the track on `StopTimeUpdate.stop_time_properties.stop_headsign`, as `Track:1`, `Track:11`, or `Track:Single`. On a test snapshot from 26 Nov 2026 (12:25 AM ET) that text was present on 44/44 trip updates and 657/679 stop-time rows, and `assigned_stop_id` was never set. This proxy reads that headsign. It does not call RailData again to do so.

`/rail/gtfs.zip` gains one platform stop per track at the stations below, mirroring the MBTA commuter-rail model ([MBTA GTFS](https://cdn.mbta.com/MBTA_GTFS.zip)): a parent stop with `location_type` 1, the original stop kept as the generic child (`platform_name` `Rail`) so existing `stop_times` rows still resolve, and one child per track with `platform_code`, `platform_name` `Rail - Track {code}`, `stop_desc` `{station} - Rail - Track {code}`, the station's coordinates, and the station's `zone_id`. Fare files are written first, so that `zone_id` is the fare area, and each platform stop is added to `stop_areas.txt`. There is no `levels.txt`, so `level_id` is left empty. Stop ids follow the Amtrak GTFS-RT convention: `105:station` and `105:track:11`. The original `stop_id` is not promoted to a station, because `stop_times` reference it and a `location_type` 1 stop cannot be used there.

The realtime feed then sets `stop_time_properties.assigned_stop_id` to that platform stop. NJ Transit's `stop_id`, times, and `stop_headsign` are left as sent. An `assigned_stop_id` NJ Transit already set is not replaced.

35 of those 44 updates were `ADDED` with an empty `trip_id`. The feed entity id is the train number and matches `trips.trip_short_name` (the same digits as `block_id`; a leading zero still matches). When exactly one scheduled trip has that number, or `start_date` selects one service in `calendar_dates.txt`, the proxy fills `trip_id`. A repeated train number with no date is left empty.

A few stations print a railroad track that is not the public label. `Single` at Summit becomes `S`, railroad `0` at Newark Airport becomes `A`, railroad `2` at Metropark becomes `1`, and Secaucus lower `1`/`2`/`3`/`4` become `G`/`F`/`H`/`E`. Any other token that is not in the checked-in list is skipped, so the zip does not grow when a new label appears.

Station-board polling (`getTrainSchedule19Rec` on `/api/TrainData`) is a fallback for stops whose headsign is not `Track:…`. It is off by default. Turn it on with `NJT_TRACK_BOARDS=on`. The default cycle is 600 seconds across the stations below, which is 1,440 TrainData calls per day, plus one TrainData `getToken` per Eastern day from that API's own budget of 10. Headsign parsing stays at zero extra calls.

| Code | Stop | Public tracks |
| --- | --- | --- |
| `NY` | New York Penn Station (`105`) | 1–21 |
| `NP` | Newark Penn Station (`107`) | A, 1–5, H |
| `SE` | Secaucus upper (`38187`) | 2, 3, A, B |
| `TS` | Secaucus lower (`38174`) | E, F, G, H |
| `HB` | Hoboken (`63`) | 1–17 |
| `TR` | Trenton Transit Center (`148`) | 1–5 |
| `NA` | Newark Airport (`37953`) | A, B |
| `MP` | Metropark (`83`) | 1, 4 |
| `ND` | Newark Broad Street (`106`) | 1–3 |
| `ST` | Summit (`145`) | 1, 2, S |

`NJT_TRACKS=off` disables platform injection, headsign assignment, and trip-id fill. `NJT_TRACK_STATIONS` is a comma-separated subset of the codes above. Stop ids above were read from the developer rail GTFS dated 2026-11-19; a row is also matched on `stop_code`.

## What it serves

| GET | Upstream | Content-Type |
| --- | --- | --- |
| `/healthz` | none | `application/json` |
| `/rail/gtfs.zip` | rail `getGTFS`, plus rail fare files from `data/fares/` | `application/zip` |
| `/rail/trip-updates.pb` | rail `getTripUpdates` | `application/x-protobuf` |
| `/rail/vehicle-positions.pb` | rail `getVehiclePositions` | `application/x-protobuf` |
| `/rail/alerts.pb` | rail `getAlerts` | `application/x-protobuf` |
| `/bus/gtfs.zip` and the three `/bus/*.pb` paths | bus G2 equivalents | same types |

A rail route exists only when both `NJT_RAIL_USERNAME` and `NJT_RAIL_PASSWORD` are set. Bus is the same with the bus variables. Anything else returns 404. `/healthz` reports which side is enabled and does not require the access key.

## Rail fares

`GET /rail/gtfs.zip` is the upstream rail zip with fare files added. The response header `x-njt-fares` is `added` when those files are present and `passthrough` when the body was not a GTFS zip. Bus zips are unchanged. Light rail routes (short names HBLR, NLR, RVLN, RVR) are not part of the rail fare network, including when the developer zip marks every route `route_type` 113. Platform stops added for tracks (`{stop_id}:station` and `{stop_id}:track:{label}`) inherit that stop's fare area.

The prices are the live njtransit.com trip planner, fetched 26 Nov 2026. The 2026 PDFs in `data/fares/source/` fill a pair or a field the planner does not price. One PDF is titled **Rail Fares 2026** (New York Penn Station, dated 2026-05-01, `published`). The Hoboken, Newark Penn, and Atlantic City PDFs are titled **Rail Proposed Fares 2026** (dated 2026-04-22, `proposed`). A row the planner priced has `source_status` `live`. A row the planner could not price keeps the PDF title, status, date, and SHA-256. Round-trip columns stay from the PDF, because the fare card has no round-trip price.

Fares are fixed. There is no peak or off-peak price, so the feed has no `timeframes.txt`.

### One area per stop

A stop has one `zone_id`. NJ Transit's printed zone number is not a fare: the same zone on the New York chart and the Hoboken chart is a different price, and an intermediate trip (Metropark to New Brunswick) is not the terminal price of either stop. Each priced GTFS stop is therefore its own area, `njt_stop_{stop_id}`, named with the stop name. Both Secaucus platforms share `njt_secaucus`. `fare_leg_rules.txt` is keyed by `network_id` `njt_rail` plus `from_area_id` / `to_area_id` for each priced pair.

`stops.zone_id` for a priced rail stop is that area id. Upstream `zone_id` values are internal ids (light rail stops share one of them) and are left in place for stops this table does not price. Fares v1 `fare_rules.txt` uses those area ids as `origin_id` and `destination_id`. v1 publishes the adult one-way price only, with `payment_method` 1 and an empty `transfers` field so the fare is the origin-to-destination ticket rather than a per-leg add-on.

### Products

For each priced pair the feed includes:

| Product | Who | Amount column |
| --- | --- | --- |
| Adult one-way | `adult` (default) | Adult one-way |
| Child, senior, and disabled one-way | three rider categories, one price | Child / senior / disabled one-way |
| 10-trip, weekly, FlexPass, monthly | `adult` | those columns |
| School monthly | `student` | School monthly |

The one-way ticket media is the NJ TRANSIT app or vending machine (same price). Round-trip amounts are kept in `data/fares/fares.csv` and are not leg products: a GTFS leg is one way, and several exception round trips are not twice the one-way fare.

`fare_leg_join_rules.txt` joins any two `njt_rail` legs that meet at the same stop. A New York–Secaucus–Trenton ride is then priced as New York to Trenton when that pair is in the table. There is no separate transfer charge in these charts, so there is no `fare_transfer_rules.txt`.

### Coverage and gaps

Priced stops are the heavy-rail stations in the public rail GTFS (`https://www.njtransit.com/rail_data.zip`, stops dated 2026-11-19) that the charts or the trip planner price. Secaucus upper and lower level share one area.

Same-line pairs, plus Hoboken to Newark-division stops and Newark Penn to Hoboken-division stops, come from `POST https://www.njtransit.com/api/graphql/graphql` (`getSchedule`). The query, variables, and the other website calls are in `docs/njtransit-web-endpoints.md`. When the planner and a PDF disagree, the planner amount is the one in the feed. `data/fares/planner_mismatches.csv` is the audit of those differences and is not what gets published. A `$0.00` pass on the site is treated as unsold: that field keeps the PDF amount when the chart has one, and is otherwise blank.

The planner refuses transfers to the Atlantic City Rail Line, and it returned no fare for Meadowlands. Those stay in `gaps.csv` with Kingsland and Monmouth Park, which are on a chart and absent from the rail GTFS. Stations beyond Suffern are not on the New York chart (it points at Metro-North tables). They do not share a trip with New York Penn, so there is no New York fare for them. Pairs they do share, including Hoboken and Newark Penn, come from the trip planner.

Refresh the CSVs after replacing a PDF:

```bash
python3 scripts/refresh_fares.py
# rematch stop ids, then fill same-line pairs from the trip planner (about one request per second):
python3 scripts/refresh_fares.py --gtfs rail_data.zip --planner
```

The script needs `pdfminer.six`. It does not call the RailData API.

## Configure

The process reads environment variables only.

| Variable | Default | Meaning |
| --- | --- | --- |
| `NJT_ENV` | `production` | `production` or `test`. Test uses `testraildata.njtransit.com` and `testpcsdata.njtransit.com`. |
| `NJT_RAIL_USERNAME` / `NJT_RAIL_PASSWORD` | unset | Rail credentials. Both are required to enable rail. |
| `NJT_BUS_USERNAME` / `NJT_BUS_PASSWORD` | unset | Bus G2 credentials. Both are required to enable bus. |
| `NJT_RAIL_BASE_URL` / `NJT_BUS_BASE_URL` | from `NJT_ENV` | Origin only, such as `https://raildata.njtransit.com`. No path, query, or userinfo. |
| `NJT_PROXY_BIND` | `127.0.0.1:8080` | Listen address. |
| `NJT_TOKEN_CACHE` | unset | JSON file for tokens. Created mode `0600` on Unix. A restart reuses the token instead of spending another rail mint. |
| `NJT_RT_TTL_SECS` | `20` | Realtime response cache. `0` disables it. Maximum 3600. |
| `NJT_STATIC_TTL_SECS` | `21600` | Static zip cache (6 hours). `0` disables it. Maximum 7 days. |
| `NJT_PROXY_ACCESS_KEY` | unset | When set, feed requests must send header `x-njt-proxy-key`. |
| `NJT_TRACKS` | `on` | `on` adds platform stops to `/rail/gtfs.zip` and fills `assigned_stop_id` from `Track:` headsigns. `off` leaves both feeds unchanged. |
| `NJT_TRACK_BOARDS` | `off` | `on` also polls TrainData `getTrainSchedule19Rec` when a headsign has no track. |
| `NJT_TRACK_STATIONS` | the ten stations in the table above | Comma-separated station codes. Unknown codes are rejected. |
| `NJT_TRACK_POLL_SECS` | `600` | Seconds for one full station-board cycle when boards are on. Allowed range 120–86400. |

Rail and bus are different hosts. Test and production credentials are issued separately. A token from one is never sent to the other.

The listen address defaults to localhost. Binding a public interface shares your NJ Transit quota and schedule data with anyone who can open the URL. If the proxy is reachable beyond the machine, set `NJT_PROXY_ACCESS_KEY`.

Tokens are minted lazily. Rail tokens are reused until the next America/New_York midnight, which is when NJ Transit resets the 10-calls-per-day `getToken` budget. Bus tokens are reused for 24 hours. Either token is replaced when a data call reports an invalid token. Minting is single-flight, persisted when `NJT_TOKEN_CACHE` is set, and hard-capped at 10 HTTP calls per API per Eastern day. A daily-limit response or the local cap becomes HTTP 503 and no further token calls are sent until midnight Eastern. `isValidToken` is never called, because it spends the same rail budget.

JSON error bodies and the literal body `Null` are treated as errors. They are not returned as zip or protobuf.

## Run locally

Rust 1.85 or newer.

```bash
export NJT_RAIL_USERNAME='your-username'
export NJT_RAIL_PASSWORD='your-password'
export NJT_TOKEN_CACHE="$HOME/.config/njt-gtfs-proxy/tokens.json"
cargo run --release --locked -p njt-gtfs-proxy
```

Add `NJT_BUS_USERNAME` and `NJT_BUS_PASSWORD` when you want bus feeds. Use `NJT_ENV=test` with the test credentials from the registration email.

## Docker

The image is multi-arch. Buildx builds each platform from the same Dockerfile.

```bash
docker buildx build --platform linux/amd64,linux/arm64 -t njt-gtfs-proxy .
docker run --rm -p 127.0.0.1:8080:8080 \
  -e NJT_RAIL_USERNAME \
  -e NJT_RAIL_PASSWORD \
  -e NJT_PROXY_ACCESS_KEY \
  -e NJT_TOKEN_CACHE=/var/lib/njt-gtfs-proxy/tokens.json \
  -v njt-tokens:/var/lib/njt-gtfs-proxy \
  njt-gtfs-proxy
```

Inside the container the server listens on `0.0.0.0:8080` so a published port works. The `-p 127.0.0.1:8080:8080` form keeps it on the host loopback. Set the access key before you publish the port on any other interface. The volume keeps the token file across restarts so a redeploy does not spend a rail `getToken` call.

## gtfs-mcp

[gtfs-mcp](https://github.com/sohampatwardhan/gtfs-mcp) [0.3.0](https://www.npmjs.com/package/gtfs-mcp) only performs plain GETs. Point it at this proxy. It reads `GTFS_MCP_STATIC`, `GTFS_MCP_TRIP_UPDATES`, `GTFS_MCP_VEHICLE_POSITIONS`, `GTFS_MCP_ALERTS`, and `GTFS_MCP_HTTP_HEADERS`. There is no `GTFS_RT_MCP_*` alias. Header values are sent on fetch and are not logged. One process registers one feed at startup, so rail and bus are two servers.

Install with `npx -y gtfs-mcp` or `cargo install gtfs-mcp --locked`, then run stdio against the proxy:

```bash
GTFS_MCP_STATIC=http://127.0.0.1:8080/rail/gtfs.zip \
GTFS_MCP_TRIP_UPDATES=http://127.0.0.1:8080/rail/trip-updates.pb \
GTFS_MCP_VEHICLE_POSITIONS=http://127.0.0.1:8080/rail/vehicle-positions.pb \
GTFS_MCP_ALERTS=http://127.0.0.1:8080/rail/alerts.pb \
GTFS_MCP_HTTP_HEADERS='{"x-njt-proxy-key":"replace-me"}' \
npx -y gtfs-mcp stdio
```

Bus uses the `/bus/` paths in the same variables. Omit `GTFS_MCP_HTTP_HEADERS` when `NJT_PROXY_ACCESS_KEY` is unset. `replace-me` is not a real key.

### Claude Desktop and Cursor

`claude_desktop_config.json` and `.cursor/mcp.json` share this `mcpServers` object:

```json
{
  "mcpServers": {
    "gtfs-njt-rail": {
      "command": "npx",
      "args": ["-y", "gtfs-mcp", "stdio"],
      "env": {
        "GTFS_MCP_STATIC": "http://127.0.0.1:8080/rail/gtfs.zip",
        "GTFS_MCP_TRIP_UPDATES": "http://127.0.0.1:8080/rail/trip-updates.pb",
        "GTFS_MCP_VEHICLE_POSITIONS": "http://127.0.0.1:8080/rail/vehicle-positions.pb",
        "GTFS_MCP_ALERTS": "http://127.0.0.1:8080/rail/alerts.pb",
        "GTFS_MCP_HTTP_HEADERS": "{\"x-njt-proxy-key\":\"replace-me\"}"
      }
    },
    "gtfs-njt-bus": {
      "command": "npx",
      "args": ["-y", "gtfs-mcp", "stdio"],
      "env": {
        "GTFS_MCP_STATIC": "http://127.0.0.1:8080/bus/gtfs.zip",
        "GTFS_MCP_TRIP_UPDATES": "http://127.0.0.1:8080/bus/trip-updates.pb",
        "GTFS_MCP_VEHICLE_POSITIONS": "http://127.0.0.1:8080/bus/vehicle-positions.pb",
        "GTFS_MCP_ALERTS": "http://127.0.0.1:8080/bus/alerts.pb",
        "GTFS_MCP_HTTP_HEADERS": "{\"x-njt-proxy-key\":\"replace-me\"}"
      }
    }
  }
}
```

### VS Code

`.vscode/mcp.json`:

```json
{
  "servers": {
    "gtfs-njt-rail": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "gtfs-mcp", "stdio"],
      "env": {
        "GTFS_MCP_STATIC": "http://127.0.0.1:8080/rail/gtfs.zip",
        "GTFS_MCP_TRIP_UPDATES": "http://127.0.0.1:8080/rail/trip-updates.pb",
        "GTFS_MCP_VEHICLE_POSITIONS": "http://127.0.0.1:8080/rail/vehicle-positions.pb",
        "GTFS_MCP_ALERTS": "http://127.0.0.1:8080/rail/alerts.pb",
        "GTFS_MCP_HTTP_HEADERS": "{\"x-njt-proxy-key\":\"replace-me\"}"
      }
    },
    "gtfs-njt-bus": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "gtfs-mcp", "stdio"],
      "env": {
        "GTFS_MCP_STATIC": "http://127.0.0.1:8080/bus/gtfs.zip",
        "GTFS_MCP_TRIP_UPDATES": "http://127.0.0.1:8080/bus/trip-updates.pb",
        "GTFS_MCP_VEHICLE_POSITIONS": "http://127.0.0.1:8080/bus/vehicle-positions.pb",
        "GTFS_MCP_ALERTS": "http://127.0.0.1:8080/bus/alerts.pb",
        "GTFS_MCP_HTTP_HEADERS": "{\"x-njt-proxy-key\":\"replace-me\"}"
      }
    }
  }
}
```

### Claude Code

```bash
claude mcp add gtfs-njt-rail \
  -e GTFS_MCP_STATIC=http://127.0.0.1:8080/rail/gtfs.zip \
  -e GTFS_MCP_TRIP_UPDATES=http://127.0.0.1:8080/rail/trip-updates.pb \
  -e GTFS_MCP_VEHICLE_POSITIONS=http://127.0.0.1:8080/rail/vehicle-positions.pb \
  -e GTFS_MCP_ALERTS=http://127.0.0.1:8080/rail/alerts.pb \
  -e GTFS_MCP_HTTP_HEADERS='{"x-njt-proxy-key":"replace-me"}' \
  -- npx -y gtfs-mcp stdio

claude mcp add gtfs-njt-bus \
  -e GTFS_MCP_STATIC=http://127.0.0.1:8080/bus/gtfs.zip \
  -e GTFS_MCP_TRIP_UPDATES=http://127.0.0.1:8080/bus/trip-updates.pb \
  -e GTFS_MCP_VEHICLE_POSITIONS=http://127.0.0.1:8080/bus/vehicle-positions.pb \
  -e GTFS_MCP_ALERTS=http://127.0.0.1:8080/bus/alerts.pb \
  -e GTFS_MCP_HTTP_HEADERS='{"x-njt-proxy-key":"replace-me"}' \
  -- npx -y gtfs-mcp stdio
```

### Codex

`~/.codex/config.toml`:

```toml
[mcp_servers.gtfs-njt-rail]
command = "npx"
args = ["-y", "gtfs-mcp", "stdio"]

[mcp_servers.gtfs-njt-rail.env]
GTFS_MCP_STATIC = "http://127.0.0.1:8080/rail/gtfs.zip"
GTFS_MCP_TRIP_UPDATES = "http://127.0.0.1:8080/rail/trip-updates.pb"
GTFS_MCP_VEHICLE_POSITIONS = "http://127.0.0.1:8080/rail/vehicle-positions.pb"
GTFS_MCP_ALERTS = "http://127.0.0.1:8080/rail/alerts.pb"
GTFS_MCP_HTTP_HEADERS = "{\"x-njt-proxy-key\":\"replace-me\"}"

[mcp_servers.gtfs-njt-bus]
command = "npx"
args = ["-y", "gtfs-mcp", "stdio"]

[mcp_servers.gtfs-njt-bus.env]
GTFS_MCP_STATIC = "http://127.0.0.1:8080/bus/gtfs.zip"
GTFS_MCP_TRIP_UPDATES = "http://127.0.0.1:8080/bus/trip-updates.pb"
GTFS_MCP_VEHICLE_POSITIONS = "http://127.0.0.1:8080/bus/vehicle-positions.pb"
GTFS_MCP_ALERTS = "http://127.0.0.1:8080/bus/alerts.pb"
GTFS_MCP_HTTP_HEADERS = "{\"x-njt-proxy-key\":\"replace-me\"}"
```

## Layout

- `crates/njt-client` — reusable rail and bus client, token manager, and mode-`0600` cache. A second binary can depend on this crate without the HTTP server.
- `crates/njt-gtfs-proxy` — axum GET server, response cache, and rail fare files added to `/rail/gtfs.zip`.
- `data/fares/` — checked-in 2026 rail fare tables and the PDFs they were read from. `scripts/refresh_fares.py` rebuilds the CSVs.

Tests talk to a local mock only. They use placeholder usernames and fake tokens.

## Upstream quirks

These differ across NJ Transit's PDF, swagger, and developer-portal samples. The client follows the swagger paths and the response fields those documents agree on.

- Rail production in the PDF and at `https://raildata.njtransit.com/swagger/GTFS/swagger.json` is `https://raildata.njtransit.com`. The developer portal's rail sample curls use `https://raildata.njt.gov`. The default here is the `.njtransit.com` host. Set `NJT_RAIL_BASE_URL` to use the other one.
- Bus `authenticateUser` returns the same JSON shape as rail `getToken`: `Authenticated` and `UserToken`. Swagger names that schema `UserAuthentication`. The bus PDF leaves the response section blank.
- One bus PDF production sample spells alerts `getALerts`. Swagger lists `/api/GTFSG2/getAlerts`, and that is the path this proxy calls.
- Bus `getGTFS` documents `{"errorMessage":"GTFS error"}`. That is a data error. It does not refresh the token. `Invalid token.` does.
- The rail PDF says to call `getToken` once per day and that the counter resets at midnight. The developer portal also describes rail tokens as valid for "hours" without a number. Cached rail tokens expire at the next America/New_York midnight, and sooner when a data call rejects them.
- `getToken` and `isValidToken` share the 10-calls-per-day rail budget. This program does not call `isValidToken`.
- Data calls are not counted against that budget, but they sometimes return HTTP 200 with JSON or the literal body `Null`. Those responses are errors.
- Rail `getGTFS` also answers GET with no token on the test and production hosts. Trip updates from the test feed put the track in `stop_headsign` (`Track:11`) and often leave `trip_id` empty on `ADDED` entities whose id is `trip_short_name`. TrainData `getTrainSchedule19Rec` is a separate token and a separate 10-calls-per-day mint budget. Current and vehicle calls on that API share a published daily cap of 40,000. This proxy does not poll it unless `NJT_TRACK_BOARDS=on`.

## Development

The trip-update types come from the `gtfs-realtime` crate, which compiles its `.proto` during the build. Install `protobuf-compiler` (`protoc`) first.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --bins --no-deps --document-private-items --locked
```

See `AGENTS.md`. Do not merge, tag, or publish from an agent.
