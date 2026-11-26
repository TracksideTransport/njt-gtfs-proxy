# njt-gtfs-proxy

Local proxy that turns NJ Transit's token-authenticated GTFS APIs into plain GET URLs.

GTFS clients only know how to GET a static zip and GTFS-Realtime protobufs. NJ Transit's rail and bus developer APIs instead require `POST` with `multipart/form-data` and a short-lived token. This process holds your credentials, caches the token, and serves the feeds.

NJ Transit's data terms apply. Register at [developer.njtransit.com](https://developer.njtransit.com/registration) and use the username and password from the email they send you. Do not commit those credentials, tokens, or downloaded feeds.

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

The prices are NJ Transit's 2026 rail fare tables in `data/fares/source/`. One PDF is titled **Rail Fares 2026** (New York Penn Station, dated 2026-05-01). The Hoboken, Newark Penn, and Atlantic City PDFs are titled **Rail Proposed Fares 2026** (dated 2026-04-22). Each fare row records that title, status, date, and SHA-256. The adjusted-fares page links all four as the charts effective July 1, 2026.

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

Same-line pairs, plus Hoboken to Newark-division stops and Newark Penn to Hoboken-division stops, are filled from `POST https://www.njtransit.com/api/graphql/graphql` (`getSchedule`). The query, variables, and the other website calls are in `docs/njtransit-web-endpoints.md`. Where a PDF row and the site disagree, the PDF row is kept. `data/fares/planner_mismatches.csv` lists the differences. The published New York one-way prices match the site except Nanuet, Pearl River, and Spring Valley (chart $19.75, site $11.55). The three Proposed charts do not match the site's one-way prices. A `$0.00` pass on the site is stored as blank, not as a free fare.

The planner refuses transfers to the Atlantic City Rail Line, and it returned no fare for Meadowlands. Those stay in `gaps.csv` with Kingsland and Monmouth Park, which are on a chart and absent from the rail GTFS. Port Jervis Line stations beyond Suffern stay unpriced: the charts point at Metro-North tables that are not in these PDFs.

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

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --bins --no-deps --document-private-items --locked
```

See `AGENTS.md`. Do not merge, tag, or publish from an agent.
