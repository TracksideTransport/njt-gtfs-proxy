# NJ Transit website endpoints

Observed in a headless browser on 26 Nov 2026 while loading `/train-to`, `/trip-planner-to` (including one Newark Penn → Trenton search), and `/station/newark-penn-station`. Analytics, ads, and Cloudflare RUM are omitted. None of these calls use the RailData developer token.

## `POST https://www.njtransit.com/api/graphql/graphql`

The site's own API. A direct call needs only `content-type: application/json` and `accept: application/json`. No cookie or API key was required. The body is a GraphQL document plus `variables`.

| Operation | Where it loaded | Variables | What comes back |
| --- | --- | --- | --- |
| `Index` → `getTrainScheduleStationsRailForSchedules`, schedule PDF lists, `getSystemStatus` | `/train-to` | none | Rail station `title`, `stationCode`, `accessible`; links to timetable PDFs; service advisories |
| `TrainSchedule` → `getSchedule` | `/train-to?origin=…&destination=…` | `origin`, `destination` (station titles), `date` (`MM/DD/YYYY`) | The fare card: `regularFare`, `reducedFare`, `tenTrip`, `weekly`, `flexPass`, `monthly`, `schoolMonthly`, plus timed trips |
| `Index` → `getTripPlannerLocations`, `getSystemStatus` | `/trip-planner-to` | none | Planner place names with lat/long and accessibility |
| `TripPlannerSchedule` → `getTripPlannerSchedule` | After Plan Your Trip | `origin`, `destination`, `timeOption` (`D` depart / arrive), `date`, `time`, `accessible`, `travelMode` (`BCTLXR` = bus, commuter rail, light rail), `maxWalkingDistance`, `minimizeTime`, optional lat/long | Itineraries. `fare.regularFare` / `reducedFare` and per-leg `legregularfare` / `legreducedfare`. Rail legs that need a transfer to the Atlantic City Rail Line are refused |

`getSchedule` is what fills `schedule-results__fare-grid` on `/train-to`. Example that matches the card for New Brunswick → Princeton Junction on 26 Nov 2026: regular `$6.60`, reduced `$2.90`, weekly `$62.00`, ten-trip `$66.00`, FlexPass `$112.20`, monthly `$206.00`, student monthly `$155.00`.

A date with no trips (Thanksgiving for some branches) returns an error string instead of a fare. The same pair on a normal weekday returns the fare. The dollar amounts do not change with the clock time.

`getTrainScheduleStationsRailForDV` (seen inside the NJ Connect payload below) adds `pentaStationID` (for example `PH` for 30th Street). That is the kind of code the station-board API uses. It is not required for the fare card.

## `GET https://njconnect.njtransit.com/api/stations`

Loaded inside the embedded trip-planner frame (`referer: https://njconnect.njtransit.com/?mode=embed&…`). A bare request without that frame returned 403. The 200 body is a combined JSON document, `data` holding:

- `getSystemStatus` — the same advisories as GraphQL
- `getTrainScheduleStationsRailForSchedules` — about 176 rail stations (`title`, `stationCode`)
- `getTrainScheduleStationsRailForDV` — the same stations plus `pentaStationID`
- `getTripPlannerLocationsHome` — a few hundred places (rail, light rail, bus)
- `getBusRoutes` — bus route names

## `GET https://njconnect.njtransit.com/api/raildata/landmarks`

Same embed referer. About 4,650 geocoder rows (`Location`, `Latitude`, `Longitude`, `Zipcode`, `Locality`, `Type`), not a rail-operations feed. The trip planner uses it to resolve typed addresses.

## `GET https://www.njtransit.com/api/chatbot`

Loaded on every page that was opened. It is the support widget, not schedule data.

## What this proxy already uses, and what it does not

The commuter-rail fare refresh calls `TrainSchedule` / `getSchedule` directly. That price is the one written into the GTFS zip. A 2026 PDF amount is used only for a pair or a field the planner does not price. It does not call RailData.

`/light-rail-to` calls the same GraphQL endpoint with `getTrainScheduleStationsLightRailForSchedules` and `getTrainSchedulesCurrentLightRail`. That station list is the three system names, and the schedules come back as PDF links, not a fare. Light rail one-way prices show up on `getTripPlannerSchedule` (`routeType` `L`). They match the flat charts: Hudson-Bergen `$2.65` / `$1.25`, Newark Light Rail and River LINE `$1.90` / `$0.85`.

`getLightRailSchedule(line, origin, destination, date)` is the light-rail schedule query in the site bundle. Its fare object is type `LightRailFare`, and the fields the bundle selects are `regularFare`, `reducedFare`, `monthly`, and `tenTrip`. On 2 Dec 2026 those calls returned the chart monthly and 10-trip amounts (Hudson-Bergen `$84.00` / `$26.50`, Newark Light Rail monthly `$71.00`, River LINE `$64.00` / `$19.00`) but `regularFare` and `reducedFare` were null. Newark Light Rail `tenTrip` came back `$0.00`, which is not the chart's `$19.00`. There is no `weekly`, `flexPass`, `schoolMonthly`, or `transferFee` on `LightRailFare`. Station titles come from `getTrainScheduleStationsLightRailForSchedulesLine(title)`.

## No bulk fare matrix

Introspection is disabled. The Nuxt bundles under `/_nuxt/` (52 chunks loaded by the home, fares, train, and light-rail pages) contain 18 GraphQL operations. None of them is a fare table, zone matrix, or spreadsheet. The fare widget only renders the per-trip `Fare` object from `getSchedule` (`regularFare`, `reducedFare`, `tenTrip`, `weekly`, `flexPass`, `monthly`, `schoolMonthly`, `transferFee`, `showMessage`). `Fare` has no `fareTable` or `zone` field.

Apollo's "Did you mean" suggestions reject the guessed names `getFareTable`, `getRailFares`, `getLightRailFares`, `getBusFares`, `getFares`, `getFareMatrix`, `getZoneFares`, `getDocument`, and `getPdf`. The nearest real fields are `getTrainLines`, `getLightRailLines` / `getLightRailLine`, `getBusRoutes`, `getPage`, and `getHomeCards`. `getLightRailLines` returns the three line titles and has no fare or zone field. `getPage(slug: "fares")` is CMS HTML that links to the adjusted-fares page. `getDocument` in the bundles is Apollo's `getDocumentInfo`, not an API field.

The chart PDFs were printed from Excel (`HBLR.xlsx`, `Newark Light Rail.xlsx`, `RiverLine.xlsx` in the PDF metadata). Those workbooks are not published next to the PDFs: the `.xlsx` URLs under `content.njtransit.com` return 404. The pair-by-pair `getSchedule` crawl is still the way to fill commuter-rail pairs the PDFs do not print.

Worth knowing, not wired in:

- `getSystemStatus` overlaps GTFS-RT alerts. The proxy already serves `/rail/alerts.pb`.
- `pentaStationID` is the station-board key. Track assignment in this proxy comes from the GTFS-RT headsign (`Track:…`) instead, which does not spend the TrainData token budget.
- `getTripPlannerSchedule` is the multimodal planner (bus and light rail included). Rail-only fares from `getSchedule` are the ones added to `/rail/gtfs.zip`.
