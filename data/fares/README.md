# Rail fare tables

These files are the NJ Transit rail fares added to `GET /rail/gtfs.zip`. They are not a download from the token-authenticated GTFS API.

## Source PDFs

`source/` holds the four official 2026 tables and the `pdftotext -layout` text beside each one. The text is for review. `scripts/refresh_fares.py` reads the PDF cell grid with pdfminer, not the text extract.

| File | PDF title | Status recorded on each row | PDF date | What it prices |
| --- | --- | --- | --- | --- |
| `source/ny-penn.pdf` | Rail Fares 2026.xlsx | `published` | 2026-05-01 | Every listed station to and from New York Penn Station |
| `source/hoboken.pdf` | Rail Proposed Fares 2026.xlsx | `proposed` | 2026-04-22 | Hoboken Division stations to and from Hoboken |
| `source/newark-penn.pdf` | Rail Proposed Fares 2026.xlsx | `proposed` | 2026-04-22 | Newark Division stations to and from Newark Penn, plus Hoboken, Secaucus, and Princeton |
| `source/atlantic-city.pdf` | Rail Proposed Fares 2026.xlsx | `proposed` | 2026-04-22 | Atlantic City Rail Line station pairs |

The Hoboken and Newark charts say those fares continue to the other terminal via the Waterfront Connection. The only explicit Hoboken–Newark Penn price in the files is the $4.20 exception on the Newark chart. Other cross-division pairs are not invented from that sentence.

SHA-256 of each PDF is stored on every fare row (`source_sha256`), with the title, status, and PDF date.

## Generated tables

| File | Role |
| --- | --- |
| `fares.csv` | One undirected terminal or station pair, with every price column and the source fields |
| `stations.csv` | Rail `stop_id`, fare area, zone number, and the chart name that matched |
| `gaps.csv` | Stops or printed names with no fare |
| `rail_stops.csv` | Heavy-rail stop directory used to match names. Refresh it with `--gtfs` when stop ids change |

Round-trip columns are in `fares.csv` only. GTFS leg products are one-way, 10-trip, weekly, FlexPass, monthly, and school monthly.

Rebuild:

```bash
python3 scripts/refresh_fares.py
python3 scripts/refresh_fares.py --gtfs /path/to/rail_data.zip
```
