#!/usr/bin/env python3
"""Rebuild the checked-in NJ Transit rail fare tables from the official PDFs.

The PDFs in ``data/fares/source/`` are the fare tables NJ Transit published for
2026. This script reads their cell grid (pdfminer) and writes:

- ``data/fares/fares.csv`` — one row per priced origin/destination pair
- ``data/fares/stations.csv`` — GTFS stop, fare area, and zone
- ``data/fares/gaps.csv`` — stations or groups with no fare in these tables
- ``data/fares/rail_stops.csv`` — rail stop directory used to match names

It does not call the RailData API and does not use credentials.

Requires ``pdfminer.six``. Optional ``--gtfs`` is a static rail GTFS zip
(for example the public ``https://www.njtransit.com/rail_data.zip``) used to
refresh ``rail_stops.csv``. Without it, the checked-in directory is reused.

Usage:
    python3 scripts/refresh_fares.py
    python3 scripts/refresh_fares.py --gtfs /path/to/rail_data.zip
    python3 scripts/refresh_fares.py --planner --gtfs /path/to/rail_data.zip

``--planner`` then prefers the njtransit.com trip planner. A price the
planner returns replaces the PDF. A blank or 0.00 planner field, and any
pair the planner cannot price, keeps the PDF.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import re
import sys
import zipfile
from collections import defaultdict
from pathlib import Path

from pdfminer.high_level import extract_pages
from pdfminer.layout import LTAnno, LTChar, LTFigure, LTRect, LTTextLine

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "data" / "fares" / "source"
OUT = ROOT / "data" / "fares"

# Metro-North stations printed on the NY and Hoboken charts but priced in
# tables 8 and 10, which are not in these PDFs.
METRO_NORTH = {
    "PORT JERVIS",
    "OTISVILLE",
    "MIDDLETOWN NY",
    "MIDDLETOWN, NY",
    "CAMPBELL HALL",
    "SALISBURY MILLS",
    "HARRIMAN",
    "TUXEDO",
    "SLOATSBURG",
}

CHARTS = {
    "ny": {
        "pdf": "ny-penn.pdf",
        "title": "Rail Fares 2026.xlsx",
        "status": "published",
        "date": "2026-05-01",
        "origin_key": "NEW YORK PENN STATION",
        "origin_name": "New York Penn Station",
        "station_x": 306.6,
        "exception_name_max_x": 470.0,
    },
    "hoboken": {
        "pdf": "hoboken.pdf",
        "title": "Rail Proposed Fares 2026.xlsx",
        "status": "proposed",
        "date": "2026-04-22",
        "origin_key": "HOBOKEN",
        "origin_name": "Hoboken Terminal",
        "station_x": 379.2,
        "exception_name_max_x": 540.0,
    },
    "newark": {
        "pdf": "newark-penn.pdf",
        "title": "Rail Proposed Fares 2026.xlsx",
        "status": "proposed",
        "date": "2026-04-22",
        "origin_key": "NEWARK PENN STATION",
        "origin_name": "Newark Penn Station",
        "station_x": 468.7,
        "exception_name_max_x": 200.0,
    },
}

PRICE_RE = re.compile(r"^\d+\.\d{2}$")
ZONE_RE = re.compile(r"^\d{1,2}$")
NOTE_RE = re.compile(
    r"SEE CHART|SEE TABLE|METRO-NORTH|NEW YORK STATIONS|TABLE 8|EXCEPTIONS|NOTE",
    re.I,
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    digest.update(path.read_bytes())
    return digest.hexdigest()


def load_page(path: Path):
    """Return page height, border positions, and text lines with characters."""
    page = next(extract_pages(str(path)))
    height = page.height
    rects = []
    lines = []

    def walk(obj):
        if isinstance(obj, LTRect):
            rects.append(obj.bbox)
        elif isinstance(obj, LTTextLine):
            chars = []
            for child in obj:
                if isinstance(child, LTChar):
                    chars.append((child.x0, child.get_text()))
                elif isinstance(child, LTAnno) and child.get_text() == " ":
                    chars.append((None, " "))
            text = obj.get_text().replace("\n", " ").strip()
            if text:
                x0, y0, x1, y1 = obj.bbox
                lines.append(
                    {
                        "x0": x0,
                        "x1": x1,
                        "top": height - y1,
                        "text": text,
                        "chars": chars,
                    }
                )
        elif isinstance(obj, LTFigure) or (
            hasattr(obj, "__iter__") and not isinstance(obj, (str, bytes))
        ):
            try:
                for child in obj:
                    walk(child)
            except TypeError:
                return

    walk(page)
    return height, rects, lines


def clustered(values, gap):
    values = sorted(values)
    merged = []
    for value in values:
        if not merged or abs(value - merged[-1]) >= gap:
            merged.append(value)
    return merged


def row_edges(height, rects):
    raw = [
        height - (y0 + y1) / 2
        for x0, y0, x1, y1 in rects
        if (y1 - y0) < 1.2 and (x1 - x0) > 80
    ]
    return clustered(raw, 1.5)


def col_edges(rects):
    raw = [
        (x0 + x1) / 2
        for x0, y0, x1, y1 in rects
        if (x1 - x0) < 1.2 and (y1 - y0) > 15
    ]
    return clustered(raw, 1.5)


def row_index(edges, top):
    rows = list(zip(edges, edges[1:]))
    for index, (start, end) in enumerate(rows):
        if start - 0.8 <= top < end - 0.8:
            return index
    return None


def money(text):
    return [token for token in text.split() if PRICE_RE.match(token)]


def is_note(text):
    return bool(NOTE_RE.search(text))


def column_slices(line, edges):
    """Split a text line only where a wide gap separates two station columns.

    Grid lines sometimes run through a merged exception cell. Cutting on every
    line would slice a name such as ``SUFFERN/MAHWAH`` in half. A real column
    change is a gap of at least 12 points between glyphs.
    """
    del edges  # Borders are not safe cut points; gaps are.
    glyphs = [(xpos, glyph) for xpos, glyph in line["chars"] if xpos is not None]
    if not glyphs:
        return [(round(line["x0"], 0), line["text"])]
    groups = [[glyphs[0]]]
    for xpos, glyph in glyphs[1:]:
        previous = groups[-1][-1][1]
        if xpos - groups[-1][-1][0] > 12 or (
            xpos - groups[-1][-1][0] > 6 and previous.isdigit() and glyph.isalpha()
        ):
            groups.append([(xpos, glyph)])
        else:
            groups[-1].append((xpos, glyph))
    if len(groups) == 1:
        return [(round(line["x0"], 0), line["text"])]
    slices = []
    for group in groups:
        text = re.sub(r"\s+", " ", "".join(glyph for _xpos, glyph in group)).strip()
        if text:
            slices.append((round(group[0][0], 0), text))
    return slices


def join_station_stack(parts):
    """Join wrapped station names and drop chart notes."""
    stations = []
    leading = []
    wrapped_words = {
        "JUNCTION",
        "STREET",
        "ST.",
        "ST",
        "AVE",
        "AVE.",
        "LEVEL",
        "NY",
        "JCT",
        "JCT.",
    }
    prefixes = {"PENN STATION", "BROAD STREET", "SECAUCUS", "SECAUCUS JCT", "MT.", "MT"}
    for text in parts:
        if is_note(text) or PRICE_RE.match(text):
            continue
        cleaned = text.strip(" *")
        if not cleaned:
            continue
        wrapped = cleaned.startswith("(") or cleaned in wrapped_words
        if wrapped and stations:
            stations[-1] = f"{stations[-1]} {cleaned}".strip()
            continue
        if wrapped:
            leading.append(cleaned)
            continue
        if stations and stations[-1] in prefixes:
            stations[-1] = f"{stations[-1]} {cleaned}".strip()
            continue
        if leading:
            cleaned = f"{cleaned} {' '.join(leading)}".strip()
            leading = []
        stations.append(cleaned)
    return stations


def explode_station(name):
    """Split a cell that holds more than one station name."""
    text = name
    if "RAMSEY" in name and "WOODCLIFF" in name:
        text = name.replace("WOODCLIFF", "|WOODCLIFF")
    parts = re.split(r"[/|]", text)
    return [part.strip(" -") for part in parts if part.strip(" -")]


def column_key(x_pos, edges):
    """Bucket a glyph x into the station column that contains it."""
    if not edges:
        return round(x_pos, 1)
    key = edges[0]
    for edge in edges:
        if x_pos + 0.5 >= edge:
            key = edge
        else:
            break
    return round(key, 1)



def _parse_terminal_cells(path: Path, chart: dict):
    height, rects, lines = load_page(path)
    edges = row_edges(height, rects)
    columns = [edge for edge in col_edges(rects) if edge >= chart["station_x"] - 2]
    by_row = defaultdict(list)
    for line in lines:
        index = row_index(edges, line["top"])
        if index is not None:
            by_row[index].append(line)

    parsed_rows = []
    for index in range(len(edges) - 1):
        row_lines = sorted(by_row[index], key=lambda line: (line["x0"], line["top"]))
        blob = " ".join(line["text"] for line in row_lines)
        zones = []
        prices = []
        station_bits = defaultdict(list)
        left_labels = []
        for line in row_lines:
            if line["x0"] < chart["station_x"] - 2:
                if ZONE_RE.match(line["text"]) and line["x0"] < 130:
                    zones.append(line["text"])
                found = money(line["text"])
                if found and line["x0"] < chart["station_x"]:
                    prices.extend(found)
                if (
                    line["x0"] < 140
                    and not ZONE_RE.match(line["text"])
                    and not PRICE_RE.match(line["text"])
                    and not is_note(line["text"])
                ):
                    left_labels.append(line["text"])
            else:
                for col, text in _slices_or_whole(line, columns):
                    if is_note(text) or PRICE_RE.match(text):
                        continue
                    station_bits[column_key(col, columns)].append(text)
        parsed_rows.append(
            {
                "index": index,
                "blob": blob,
                "zone": zones[0] if zones else None,
                "prices": prices,
                "bits": station_bits,
                "left": left_labels,
                "exception_header": bool(re.search(r"EXCEPTIONS TO ABOVE", blob, re.I)),
            }
        )

    records = []
    exception_mode = False
    span_bits = defaultdict(list)
    span_zone = None
    span_prices = None

    def emit(zone, prices, bits, exception, left):
        names = []
        for col in sorted(bits):
            names.extend(join_station_stack(bits[col]))
        if left and exception:
            # Newark labels the exception row in the zone column.
            extra = join_station_stack(left)
            names = extra + [name for name in names if name not in extra]
        names = [name for name in names if not is_note(name)]
        if not names or not prices:
            return
        if len(prices) != 9:
            raise SystemExit(
                f"{path.name} zone {zone}: expected 9 price columns, found {prices} for {names}"
            )
        # Slash-separated exception groups are several stations at one price.
        split_names = []
        for name in names:
            split_names.extend(explode_station(name))
        records.append(
            {
                "zone": zone or "",
                "prices": prices,
                "stations": split_names,
                "exception": exception,
            }
        )

    for row in parsed_rows:
        if row["exception_header"]:
            # Secaucus upper level shares the exceptions banner row on the
            # New York chart. It is still part of the zone above the banner.
            for col, texts in row["bits"].items():
                span_bits[col].extend(texts)
            emit(span_zone, span_prices, span_bits, False, [])
            span_bits = defaultdict(list)
            span_prices = None
            span_zone = None
            exception_mode = True
            continue
        if not exception_mode:
            if row["zone"]:
                emit(span_zone, span_prices, span_bits, False, [])
                span_bits = defaultdict(list)
                span_zone = row["zone"]
                span_prices = row["prices"] or span_prices
            elif row["prices"] and not span_prices:
                span_prices = row["prices"]
            for col, texts in row["bits"].items():
                span_bits[col].extend(texts)
        else:
            if row["prices"]:
                bits = defaultdict(list)
                for col, texts in row["bits"].items():
                    # Drop the repeated "PENN STATION / NEW YORK" footer label.
                    kept = [
                        text
                        for text in texts
                        if text.strip().upper() not in {"PENN STATION", "NEW YORK", "NEWARK", "TERMINAL"}
                        or row["left"]
                    ]
                    bits[col].extend(kept)
                emit(row["zone"] or "", row["prices"], bits, True, row["left"])
    emit(span_zone, span_prices, span_bits, False, [])
    return records


def _slices_or_whole(line, columns):
    if len(columns) < 2:
        return [(0, line["text"])]
    slices = column_slices(line, columns)
    if len(slices) <= 1:
        return [(round(line["x0"], 0), line["text"])]
    return slices


def norm_name(text: str) -> str:
    name = text.upper().replace("&", " AND ")
    name = name.replace(".", " ").replace(",", " ").replace("-", " ").replace("/", " ")
    name = name.replace("(", " ").replace(")", " ")
    name = re.sub(r"\s+", " ", name).strip()
    aliases = {
        "BAY HEAD JCT": "BAY HEAD",
        "PT PLEASANT": "POINT PLEASANT",
        "MATAWAN": "ABERDEEN MATAWAN",
        "METRO PARK": "METROPARK",
        "JERSEY AVE": "JERSEY AVE",
        "PENN STATION NEWARK": "NEWARK PENN STATION",
        "BROAD STREET NEWARK": "NEWARK BROAD ST",
        "WATSESSING RIDGEWOOD AVE": "WATSESSING AVENUE",
        "NEWARK PENN": "NEWARK PENN STATION",
        "PENN STATION NEW YORK": "NEW YORK PENN STATION",
        "NEW YORK PENN": "NEW YORK PENN STATION",
        "HOBOKEN TERMINAL": "HOBOKEN",
        "UNION STATION": "UNION",
        "NO ELIZABETH": "NORTH ELIZABETH",
        "SO ORANGE": "SOUTH ORANGE",
        "MOUNTAIN STA": "MOUNTAIN STATION",
        "HIGHLAND AVE": "HIGHLAND AVENUE",
        "WATCHUNG AVE": "WATCHUNG AVENUE",
        "WALNUT ST": "WALNUT STREET",
        "BAY ST": "BAY STREET",
        "ANDERSON ST": "ANDERSON STREET",
        "ESSEX ST": "ESSEX STREET",
        "NEW BRIDGE LANDG": "NEW BRIDGE LANDING",
        "HO HO KUS": "HOHOKUS",
        "MT ARLINGTON": "MOUNT ARLINGTON",
        "MT OLIVE": "MOUNT OLIVE",
        "NORTHBRANCH": "NORTH BRANCH",
        "PRINCETON JCT": "PRINCETON JCT",
        "PRINCETON JUNCTION": "PRINCETON JCT",
        "MONTCLAIR STATE U": "MSU",
        "MONTCLAIR STATE UNIV": "MSU",
        "WAYNE RT 23": "WAYNE ROUTE 23 TRANSIT CENTER",
        "EDISON": "EDISON STATION",
        "MOUNTAIN AVE": "MOUNTAIN AVENUE",
        "WATSESSING": "WATSESSING AVENUE",
        "MIDDLETOWN": "MIDDLETOWN NJ",
        "RAMSEY RT 17": "RAMSEY ROUTE 17 STATION",
        "RAMSEY ROUTE 17": "RAMSEY ROUTE 17 STATION",
        "FAIRLAWN BRDWY": "BROADWAY",
        "BROADWAY FAIRLAWN": "BROADWAY",
        "FAIRLAWN FAIRLAWN": "BROADWAY",
        "BROADWAY FAIRLAWN FAIRLAWN": "BROADWAY",
        "GLEN ROCK BORO HALL": "GLEN ROCK BORO HALL",
        "GLEN ROCK MAIN LINE": "GLEN ROCK MAIN LINE",
        "BLOOMFIELD RIDGEWOOD AVE": "BLOOMFIELD",
        "RIDGEWOOD AVE": "BLOOMFIELD",
        "PHILADELPHIA": "30TH ST PHL",
        "30TH ST PHL": "30TH ST PHL",
        "EGG HARBOR CITY": "EGG HARBOR",
        "SECAUCUS JCT": "SECAUCUS",
        "SECAUCUS JCT LOWER LEVEL": "SECAUCUS",
        "SECAUCUS LOWER LEVEL": "SECAUCUS",
        "SECAUCUS UPPER LEVEL": "SECAUCUS",
        "SECAUCUS": "SECAUCUS",
        "BORO HALL": "GLEN ROCK BORO HALL",
        "MAIN LINE": "GLEN ROCK MAIN LINE",
        "SUFFERN NY": "SUFFERN",
        "SUFFERN,NY": "SUFFERN",
    }
    return aliases.get(name, name)


def norm_stop(text: str) -> str:
    name = text.upper().replace(".", " ").replace(",", " ").replace("-", " ")
    name = name.replace("/", " ").replace("[", " ").replace("]", " ")
    name = re.sub(r"\s+", " ", name).strip()
    name = re.sub(r"\s+RR$", "", name).strip()
    return name


def load_rail_stops(path: Path):
    stops = list(csv.DictReader(path.open()))
    return stops


def refresh_rail_stops(gtfs_zip: Path, dest: Path):
    with zipfile.ZipFile(gtfs_zip) as archive:
        routes = {
            row["route_id"]: row
            for row in csv.DictReader(archive.read("routes.txt").decode().splitlines())
        }
        rail_routes = {
            route_id
            for route_id, row in routes.items()
            if row.get("route_type") == "2"
        }
        trips = csv.DictReader(archive.read("trips.txt").decode().splitlines())
        trip_route = {
            row["trip_id"]: row["route_id"]
            for row in trips
            if row["route_id"] in rail_routes
        }
        seen = defaultdict(set)
        for row in csv.DictReader(archive.read("stop_times.txt").decode().splitlines()):
            route_id = trip_route.get(row["trip_id"])
            if route_id:
                seen[row["stop_id"]].add(routes[route_id]["route_short_name"])
        stop_rows = []
        for row in csv.DictReader(archive.read("stops.txt").decode().splitlines()):
            if row["stop_id"] not in seen:
                continue
            stop_rows.append(
                {
                    "stop_id": row["stop_id"],
                    "stop_name": row["stop_name"],
                    "routes": " ".join(sorted(seen[row["stop_id"]])),
                }
            )
    stop_rows.sort(key=lambda row: int(row["stop_id"]))
    write_csv(dest, ["stop_id", "stop_name", "routes"], stop_rows)
    return stop_rows


def write_csv(path: Path, fields, rows):
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields, lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


def match_stops(stops):
    by_norm = defaultdict(list)
    for stop in stops:
        by_norm[norm_stop(stop["stop_name"])].append(stop)
    # Disambiguate the two Trenton / Hamilton / Pennsauken stops by route.
    return by_norm


SPECIAL_STOP = {
    "TRENTON": "148",
    "HAMILTON": "32905",
    "PENNSAUKEN": "43298",
    "JERSEY AVE": "32906",
}


def resolve_stop(key, by_norm, stops_by_id):
    if key in SPECIAL_STOP:
        return stops_by_id[SPECIAL_STOP[key]]
    found = by_norm.get(key, [])
    if len(found) == 1:
        return found[0]
    return None


def parse_acrl(path: Path):
    """Atlantic City chart is a station-pair matrix, not a zone grid.

    Each block is "Fares to/from {origin} and" followed by destination rows.
    The origin name sits above the word ``and``; each destination row has the
    station name and six prices (one-way, reduced, weekly, FlexPass, monthly,
    school monthly). There is no 10-trip or round-trip column.
    """
    _height, _rects, lines = load_page(path)
    grouped = defaultdict(list)
    for line in lines:
        grouped[round(line["top"] / 2) * 2].append(line)
    header_words = {
        "FARES",
        "TO/FROM",
        "CHILD",
        "ADULT",
        "SR/DIS",
        "SCHOOL",
        "ONE",
        "WAY",
        "WEEKLY",
        "FLEXPASS",
        "MONTHLY",
        "AND",
    }
    origin = None
    pending = None
    pairs = []
    for top in sorted(grouped):
        items = sorted(grouped[top], key=lambda line: line["x0"])
        names = [
            line["text"].strip()
            for line in items
            if line["x0"] < 250 and not PRICE_RE.match(line["text"].strip())
        ]
        prices = []
        for line in items:
            prices.extend(money(line["text"]))
        blob = " ".join(line["text"] for line in items)
        name = " ".join(names).strip()
        name = " ".join(
            token
            for token in re.sub(r"\s+", " ", name).split()
            if token.upper() not in header_words and token.upper() not in {"SR/DIS", "TO", "FROM"}
        )
        if re.search(r"\band\b", blob, re.I) and pending:
            origin = pending
            pending = None
            continue
        if not name or name.upper() in header_words:
            continue
        if "RAIL LINE" in name.upper() or name.upper().startswith("FARES"):
            continue
        if prices and len(prices) == 6 and origin:
            pairs.append((origin, name, prices))
            continue
        if not prices and "Adult" not in blob and "One Way" not in blob:
            pending = name
            continue
        if "Adult" in blob and not prices:
            pending = name
    if len(pairs) < 36:
        rendered = "\n".join(f"{a} -> {b} {p[0]}" for a, b, p in pairs)
        raise SystemExit(f"Atlantic City parser found only {len(pairs)} pairs\n{rendered}")
    return pairs


def price_fields(values, kind):
    if kind == "terminal":
        keys = [
            "adult_ow",
            "reduced_ow",
            "adult_rt",
            "child_rt",
            "ten_trip",
            "weekly",
            "flexpass",
            "monthly",
            "school_monthly",
        ]
    else:
        keys = [
            "adult_ow",
            "reduced_ow",
            "weekly",
            "flexpass",
            "monthly",
            "school_monthly",
        ]
    return dict(zip(keys, values))


def expand_key(key: str):
    """One printed name can price both platforms or both Glen Rock stops."""
    if key == "GLEN ROCK":
        return ["GLEN ROCK BORO HALL", "GLEN ROCK MAIN LINE"]
    return [key]


def slug(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "_", text.lower()).strip("_")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--gtfs",
        type=Path,
        help="Public NJT rail GTFS zip. Refreshes data/fares/rail_stops.csv.",
    )
    parser.add_argument(
        "--planner",
        action="store_true",
        help="After the PDF tables, apply trip-planner prices. The planner wins; a blank planner field keeps the PDF.",
    )
    parser.add_argument(
        "--planner-limit",
        type=int,
        default=0,
        help="With --planner, fetch at most this many station-code pairs.",
    )
    parser.add_argument(
        "--planner-dry-run",
        action="store_true",
        help="With --planner, print pair counts and exit before any fare request.",
    )
    args = parser.parse_args()
    stops_path = OUT / "rail_stops.csv"
    if args.gtfs:
        stops = refresh_rail_stops(args.gtfs, stops_path)
    elif stops_path.exists():
        stops = load_rail_stops(stops_path)
    else:
        raise SystemExit("rail_stops.csv is missing; pass --gtfs")

    by_norm = match_stops(stops)
    by_id = {stop["stop_id"]: stop for stop in stops}

    # station key -> chart -> price record
    station_prices = defaultdict(dict)
    metro_north_hits = set()
    unmatched = []

    for chart_id, chart in CHARTS.items():
        pdf = SOURCE / chart["pdf"]
        chart["sha256"] = sha256(pdf)
        records = _parse_terminal_cells(pdf, chart)
        for record in records:
            fields = price_fields(record["prices"], "terminal")
            for raw_name in record["stations"]:
                key = norm_name(raw_name)
                if key in {norm_name(name) for name in METRO_NORTH} or raw_name.upper() in METRO_NORTH:
                    metro_north_hits.add(raw_name.upper())
                    continue
                if key in {"TERMINAL", "RIDGEWOOD AVE"}:
                    continue
                if key == chart["origin_key"] and not record["exception"]:
                    continue
                if key == "NEWARK PENN STATION" and chart_id == "newark":
                    continue
                if key == "HOBOKEN" and chart_id == "hoboken":
                    continue
                targets = expand_key(key)
                for target in targets:
                    station_prices[target][chart_id] = {
                        "zone": record["zone"],
                        "exception": "1" if record["exception"] else "0",
                        **fields,
                        "raw": raw_name,
                    }

    acrl_pdf = SOURCE / "atlantic-city.pdf"
    acrl_sha = sha256(acrl_pdf)
    acrl_pairs = parse_acrl(acrl_pdf)

    # Resolve keys to stops. Secaucus is one fare point for both platforms.
    key_stops = {}
    for key in list(station_prices):
        stop = resolve_stop(key, by_norm, by_id)
        if stop is None and key == "SECAUCUS":
            key_stops[key] = [
                by_id["38174"],
                by_id["38187"],
            ]
            continue
        if stop is None and key in {"KINGSLAND", "MONMOUTH PARK"}:
            continue
        if stop is None:
            unmatched.append(key)
            continue
        key_stops[key] = [stop]

    if unmatched:
        print("UNMATCHED", file=sys.stderr)
        for key in sorted(set(unmatched)):
            print(f"  {key}", file=sys.stderr)
        raise SystemExit(f"{len(set(unmatched))} chart names did not match a rail stop")

    # Signature for area clustering. Both Secaucus platforms share one signature.
    def signature(key):
        parts = []
        zones = []
        exception = False
        charts = []
        for chart_id in ("ny", "hoboken", "newark"):
            row = station_prices[key].get(chart_id)
            if not row:
                continue
            charts.append(chart_id)
            if row["exception"] == "1":
                exception = True
            if row["zone"]:
                zones.append(int(row["zone"]))
            parts.append(
                (
                    chart_id,
                    row["exception"],
                    row["adult_ow"],
                    row["reduced_ow"],
                    row["weekly"],
                    row["monthly"],
                    row["school_monthly"],
                    row["ten_trip"],
                    row["flexpass"],
                    row["adult_rt"],
                    row["child_rt"],
                )
            )
        zone = zones[0] if zones and all(item == zones[0] for item in zones) else None
        if zones and zone is None:
            # Prefer the New York chart's zone number when the charts differ.
            ny = station_prices[key].get("ny")
            zone = int(ny["zone"]) if ny and ny["zone"] else zones[0]
        return zone, exception, tuple(charts), tuple(parts)

    groups = defaultdict(list)
    for key in station_prices:
        groups[signature(key)].append(key)

    zone_group_count = defaultdict(int)
    for (zone, exception, _charts, _parts), _keys in groups.items():
        if zone is not None and not exception:
            zone_group_count[zone] += 1

    area_of_key = {}
    area_names = {}
    for (zone, exception, charts, _parts), keys in groups.items():
        keys = sorted(keys)
        if keys == ["SECAUCUS"]:
            area_id = "njt_zone_1"
            area_name = "Rail zone 1"
            for key in keys:
                area_of_key[key] = area_id
                area_names[area_id] = area_name
            continue
        if exception:
            area_id = "njt_exc_" + slug("_".join(keys))[:48].strip("_")
            label = " / ".join(title_name(key) for key in keys)
            zone_bit = f", zone {zone}" if zone else ""
            area_name = f"{label} (exception{zone_bit})"
        elif zone is not None and zone_group_count[zone] == 1:
            area_id = f"njt_zone_{zone}"
            area_name = f"Rail zone {zone}"
        else:
            if charts == ("ny", "hoboken"):
                suffix = "hoboken_div"
                div = "Hoboken division"
            elif charts == ("ny", "newark"):
                suffix = "newark_div"
                div = "Newark division"
            else:
                suffix = "_".join(charts)
                div = ", ".join(charts)
            area_id = f"njt_zone_{zone}_{suffix}" if zone is not None else f"njt_{suffix}_" + slug(keys[0])
            area_name = f"Rail zone {zone} ({div})" if zone is not None else div
        for key in keys:
            area_of_key[key] = area_id
            area_names[area_id] = area_name

    # Terminals are their own areas even when a chart also lists them in a zone.
    terminal_areas = {
        "NEW YORK PENN STATION": ("njt_nyp", "New York Penn Station"),
        "HOBOKEN": ("njt_hoboken", "Hoboken Terminal"),
        "NEWARK PENN STATION": ("njt_newark_penn", "Newark Penn Station"),
    }
    for key, (area_id, area_name) in terminal_areas.items():
        area_of_key[key] = area_id
        area_names[area_id] = area_name

    acrl_area = {}

    def acrl_key(name):
        return norm_name(name)

    acrl_names = set()
    for origin, dest, _prices in acrl_pairs:
        acrl_names.add(acrl_key(origin))
        acrl_names.add(acrl_key(dest))
    for key in sorted(acrl_names):
        area_id = "njt_ac_" + slug(key)
        acrl_area[key] = area_id
        area_names[area_id] = title_name(key)
        stop = resolve_stop(key, by_norm, by_id)
        if stop is None:
            raise SystemExit(f"Atlantic City station did not match GTFS: {key}")
        key_stops[key] = [stop]

    fare_rows = []

    def add_fare(origin_key, dest_key, zone, exception, prices, source):
        fare_rows.append(
            {
                "origin_area_id": area_of_key[origin_key],
                "destination_area_id": area_of_key[dest_key],
                "origin_name": area_names[area_of_key[origin_key]],
                "destination_name": area_names[area_of_key[dest_key]],
                "zone": zone,
                "is_exception": exception,
                "adult_ow": prices["adult_ow"],
                "reduced_ow": prices["reduced_ow"],
                "adult_rt": prices.get("adult_rt", ""),
                "child_rt": prices.get("child_rt", ""),
                "ten_trip": prices.get("ten_trip", ""),
                "weekly": prices["weekly"],
                "flexpass": prices["flexpass"],
                "monthly": prices["monthly"],
                "school_monthly": prices["school_monthly"],
                "source_file": source["file"],
                "source_title": source["title"],
                "source_status": source["status"],
                "source_date": source["date"],
                "source_sha256": source["sha"],
            }
        )

    for key, charts_for_key in sorted(station_prices.items()):
        if key in terminal_areas and key != "NEWARK PENN STATION" and key != "HOBOKEN":
            pass
        for chart_id, row in sorted(charts_for_key.items()):
            origin_key = CHARTS[chart_id]["origin_key"]
            if key == origin_key:
                continue
            # A terminal listed inside another chart keeps the terminal area.
            add_fare(
                origin_key,
                key,
                row["zone"],
                row["exception"],
                row,
                {
                    "file": CHARTS[chart_id]["pdf"],
                    "title": CHARTS[chart_id]["title"],
                    "status": CHARTS[chart_id]["status"],
                    "date": CHARTS[chart_id]["date"],
                    "sha": CHARTS[chart_id]["sha256"],
                },
            )

    for origin, dest, prices in acrl_pairs:
        origin_key = acrl_key(origin)
        dest_key = acrl_key(dest)
        fields = price_fields(prices, "acrl")
        fare_rows.append(
            {
                "origin_area_id": acrl_area[origin_key],
                "destination_area_id": acrl_area[dest_key],
                "origin_name": area_names[acrl_area[origin_key]],
                "destination_name": area_names[acrl_area[dest_key]],
                "zone": "",
                "is_exception": "0",
                "adult_ow": fields["adult_ow"],
                "reduced_ow": fields["reduced_ow"],
                "adult_rt": "",
                "child_rt": "",
                "ten_trip": "",
                "weekly": fields["weekly"],
                "flexpass": fields["flexpass"],
                "monthly": fields["monthly"],
                "school_monthly": fields["school_monthly"],
                "source_file": "atlantic-city.pdf",
                "source_title": "Rail Proposed Fares 2026.xlsx",
                "source_status": "proposed",
                "source_date": "2026-04-22",
                "source_sha256": acrl_sha,
            }
        )

    # stations.csv — one row per priced stop.
    station_rows = []
    seen_stops = set()

    def add_station(stop, area_id, zone, chart_names):
        if stop["stop_id"] in seen_stops:
            return
        seen_stops.add(stop["stop_id"])
        station_rows.append(
            {
                "stop_id": stop["stop_id"],
                "stop_name": stop["stop_name"],
                "area_id": area_id,
                "area_name": area_names[area_id],
                "zone": "" if zone is None else str(zone),
                "routes": stop["routes"],
                "chart_names": " | ".join(chart_names),
            }
        )

    for key, stop_list in key_stops.items():
        if key not in area_of_key and key not in acrl_area:
            continue
        area_id = area_of_key.get(key) or acrl_area[key]
        zone, _exception, _charts, _parts = signature(key) if key in station_prices else (None, False, (), ())
        if key in terminal_areas:
            zone = None
        names = []
        if key in station_prices:
            names = sorted({row["raw"] for row in station_prices[key].values()})
        elif key in acrl_area:
            names = [key]
        for stop in stop_list:
            add_station(stop, area_id, zone, names)

    for key, (area_id, _area_name) in terminal_areas.items():
        stop = resolve_stop(key, by_norm, by_id)
        if stop is None:
            raise SystemExit(f"terminal {key} did not match a stop")
        add_station(stop, area_id, None, [key])
    station_rows.sort(key=lambda row: (row["area_id"], int(row["stop_id"])))

    fare_rows.sort(
        key=lambda row: (
            row["source_file"],
            row["origin_area_id"],
            row["destination_area_id"],
            row["adult_ow"],
        )
    )
    deduped = []
    seen_pairs = set()
    for row in fare_rows:
        pair = (row["source_file"], row["origin_area_id"], row["destination_area_id"])
        if pair in seen_pairs:
            continue
        seen_pairs.add(pair)
        deduped.append(row)
    fare_rows = deduped

    gaps = []
    priced_ids = {row["stop_id"] for row in station_rows}
    for stop in stops:
        if stop["stop_id"] in priced_ids:
            continue
        name = stop["stop_name"].upper()
        if any(token in name for token in ("PORT JERVIS", "OTISVILLE", "CAMPBELL", "SALISBURY", "HARRIMAN", "TUXEDO", "SLOATSBURG", "MIDDLETOWN NY")):
            reason = "metro_north_tables_8_and_10_not_in_source_pdfs"
        elif "AIRPORT" in name:
            reason = "newark_airport_not_in_fare_tables"
        elif "MEADOWLANDS" in name:
            reason = "meadowlands_event_service_not_in_fare_tables"
        else:
            reason = "no_row_in_the_2026_fare_tables"
        gaps.append(
            {
                "stop_id": stop["stop_id"],
                "stop_name": stop["stop_name"],
                "routes": stop["routes"],
                "reason": reason,
            }
        )
    for name in sorted(METRO_NORTH):
        if name in {"MIDDLETOWN, NY", "MIDDLETOWN NY"}:
            continue
    gaps.append(
        {
            "stop_id": "",
            "stop_name": "Kingsland",
            "routes": "",
            "reason": "printed_in_zone_2_but_absent_from_rail_gtfs",
        }
    )
    gaps.append(
        {
            "stop_id": "",
            "stop_name": "Monmouth Park",
            "routes": "",
            "reason": "printed_in_zone_19_but_absent_from_rail_gtfs",
        }
    )
    gaps.sort(key=lambda row: (row["reason"], row["stop_name"]))

    write_csv(
        OUT / "fares.csv",
        [
            "origin_area_id",
            "destination_area_id",
            "origin_name",
            "destination_name",
            "zone",
            "is_exception",
            "adult_ow",
            "reduced_ow",
            "adult_rt",
            "child_rt",
            "ten_trip",
            "weekly",
            "flexpass",
            "monthly",
            "school_monthly",
            "source_file",
            "source_title",
            "source_status",
            "source_date",
            "source_sha256",
        ],
        fare_rows,
    )
    write_csv(
        OUT / "stations.csv",
        ["stop_id", "stop_name", "area_id", "area_name", "zone", "routes", "chart_names"],
        station_rows,
    )
    write_csv(
        OUT / "gaps.csv",
        ["stop_id", "stop_name", "routes", "reason"],
        gaps,
    )
    check_goldens(fare_rows, station_rows)
    if args.planner:
        if args.gtfs is None:
            raise SystemExit("--planner needs --gtfs so same-line pairs come from the rail zip")
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        import planner_fares

        planner_fares.run(args.gtfs, limit=args.planner_limit, dry_run=args.planner_dry_run)
        return 0
    print(
        f"fares={len(fare_rows)} stations={len(station_rows)} "
        f"areas={len({row['area_id'] for row in station_rows})} gaps={len(gaps)} "
        f"metro_north_labels={sorted(metro_north_hits)}"
    )
    return 0


def title_name(key: str) -> str:
    special = {
        "NEW YORK PENN STATION": "New York Penn Station",
        "NEWARK PENN STATION": "Newark Penn Station",
        "HOBOKEN": "Hoboken",
        "30TH ST PHL": "30th Street Philadelphia",
        "PRINCETON JCT": "Princeton Junction",
        "MSU": "Montclair State University",
        "JERSEY AVE": "Jersey Avenue",
    }
    if key in special:
        return special[key]
    return " ".join(part.capitalize() for part in key.split())


def lookup(rows, origin, dest, source):
    found = [
        row
        for row in rows
        if row["source_file"] == source
        and row["origin_name"] == origin
        and dest in row["destination_name"]
    ]
    return found


def check_goldens(fares, stations):
    def one(source, origin, dest_substr):
        found = lookup(fares, origin, dest_substr, source)
        if len(found) != 1:
            names = [row["destination_name"] for row in found]
            raise SystemExit(f"golden miss {source} {origin} -> {dest_substr}: {names}")
        return found[0]

    trenton = one("ny-penn.pdf", "New York Penn Station", "zone 22")
    # Trenton shares a zone area; the adult fare is the zone 22 price.
    if trenton["adult_ow"] != "20.35" or trenton["monthly"] != "585.00":
        raise SystemExit(f"Trenton/NY zone 22 fare drifted: {trenton['adult_ow']}")
    suffern = one("ny-penn.pdf", "New York Penn Station", "Suffern")
    if suffern["adult_ow"] != "14.80" or suffern["adult_rt"] != "23.80" or suffern["is_exception"] != "1":
        raise SystemExit(f"Suffern exception drifted: {suffern['adult_ow']} {suffern['adult_rt']}")
    princeton = one("ny-penn.pdf", "New York Penn Station", "Princeton (exception")
    if princeton["adult_ow"] != "21.60":
        raise SystemExit(f"Princeton exception drifted: {princeton['adult_ow']}")
    hoboken_wood = one("hoboken.pdf", "Hoboken Terminal", "zone 3")
    if hoboken_wood["adult_ow"] != "5.05":
        raise SystemExit(f"Hoboken zone 3 drifted: {hoboken_wood['adult_ow']}")
    newark_trenton = one("newark-penn.pdf", "Newark Penn Station", "zone 22")
    if newark_trenton["adult_ow"] != "15.15":
        raise SystemExit(f"Newark zone 22 drifted: {newark_trenton['adult_ow']}")
    hoboken_link = one("newark-penn.pdf", "Newark Penn Station", "Hoboken")
    if hoboken_link["adult_ow"] != "4.20":
        raise SystemExit(f"Newark-Hoboken drifted: {hoboken_link['adult_ow']}")
    ac = [
        row
        for row in fares
        if row["source_file"] == "atlantic-city.pdf"
        and "Atlantic" in row["origin_name"]
        and "30th" in row["destination_name"]
    ]
    if len(ac) != 1 or ac[0]["adult_ow"] != "13.05" or ac[0]["ten_trip"] != "":
        raise SystemExit(f"ACRL Philadelphia fare drifted: {ac}")
    secaucus = [row for row in stations if row["stop_name"].startswith("SECAUCUS")]
    if len(secaucus) != 2 or len({row["area_id"] for row in secaucus}) != 1:
        raise SystemExit(f"Secaucus platforms should share one area: {secaucus}")
    nyp = [row for row in stations if row["stop_id"] == "105"]
    if not nyp or nyp[0]["area_id"] != "njt_nyp":
        raise SystemExit("New York Penn stop 105 was not its own area")
    print("golden checks ok")


if __name__ == "__main__":
    sys.exit(main())
