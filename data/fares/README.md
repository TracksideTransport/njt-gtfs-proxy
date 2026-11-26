# Rail fare tables

These files are the NJ Transit rail fares added to `GET /rail/gtfs.zip`. They are not a download from the token-authenticated GTFS API.

## Source PDFs

`source/` holds the four official 2026 tables and the `pdftotext -layout` text beside each one. The text is for review. `scripts/refresh_fares.py` reads the PDF cell grid with pdfminer, not the text extract.

| File | PDF title | Status when the planner has no price | PDF date | What it prices |
| --- | --- | --- | --- | --- |
| `source/ny-penn.pdf` | Rail Fares 2026.xlsx | `published` | 2026-05-01 | Every listed station to and from New York Penn Station |
| `source/hoboken.pdf` | Rail Proposed Fares 2026.xlsx | `proposed` | 2026-04-22 | Hoboken Division stations to and from Hoboken |
| `source/newark-penn.pdf` | Rail Proposed Fares 2026.xlsx | `proposed` | 2026-04-22 | Newark Division stations to and from Newark Penn, plus Hoboken, Secaucus, and Princeton |
| `source/atlantic-city.pdf` | Rail Proposed Fares 2026.xlsx | `proposed` | 2026-04-22 | Atlantic City Rail Line station pairs |

The Hoboken and Newark charts say those fares continue to the other terminal via the Waterfront Connection. The only explicit Hoboken–Newark Penn price in the files is the $4.20 exception on the Newark chart. Other cross-division pairs are not invented from that sentence.

A row the planner priced uses `source_status` `live` and an empty `source_sha256`. A row the planner could not price keeps the PDF title, status, date, and SHA-256.

## Generated tables

| File | Role |
| --- | --- |
| `fares.csv` | One undirected terminal or station pair, with every price column and the source fields |
| `stations.csv` | Rail `stop_id`, fare area, zone number, and the chart name that matched |
| `gaps.csv` | Stops with no fare, and station pairs that are not fare rules. The `reason` column separates `no single through ticket` (off the Atlantic City Rail Line; the River LINE connects, each leg priced separately), `meadowlands has no regular fare`, and `no trip` |
| `rail_stops.csv` | Heavy-rail stop directory used to match names. Refresh it with `--gtfs` when stop ids change |

Round-trip columns are in `fares.csv` only. GTFS leg products are one-way, 10-trip, weekly, FlexPass, monthly, and school monthly.

## Trip planner

`--planner` calls `POST https://www.njtransit.com/api/graphql/graphql` (`getSchedule`) for same-line station pairs and for Hoboken↔Newark-division pairs. Responses are cached under `planner_cache/` (gitignored) and the job resumes. The planner price replaces the PDF. A blank or `0.00` planner field keeps the PDF amount for that field. A pair the planner cannot price keeps the PDF row. `planner_mismatches.csv` records every field where the two sources differ. It is an audit, not the published table.

Each priced commuter-rail stop is its own area (`njt_stop_{stop_id}`). Secaucus upper and lower share `njt_secaucus`. A printed zone number is not an area: the same zone is a different price from New York and from Hoboken, and an intermediate trip is not the terminal price.

Pairs that leave the Atlantic City Rail Line are `no single through ticket` in `gaps.csv` and are not fare rules. The River LINE connects that line to the Northeast Corridor at Pennsauken and Trenton, and each leg is priced separately (Atlantic City fare, River LINE fare, Northeast Corridor fare). Philadelphia 30th Street is an Atlantic City station that SEPTA also serves; no other NJ Transit rail line serves it. Trenton Transit Center is the other station shared with SEPTA, and there is no joint SEPTA fare. Meadowlands pairs are `meadowlands has no regular fare`. Pairs with no planner trip are `no trip`.

## Light rail

`light_rail.csv` is the flat fare for Hudson-Bergen Light Rail, Newark Light Rail, and the River LINE, plus the Newark underground one-way. The PDFs are `source/hblr.pdf`, `source/newark-light-rail.pdf`, and `source/river-line.pdf`. The proxy puts these on network `njt_light_rail`, not `njt_rail`.

Rebuild:

```bash
python3 scripts/refresh_fares.py
python3 scripts/refresh_fares.py --gtfs /path/to/rail_data.zip --planner
```
