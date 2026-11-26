#!/usr/bin/env python3
"""Fill same-line and listed gap fares from the NJ Transit trip planner.

The train-to page loads fares from ``POST /api/graphql/graphql`` operation
``getSchedule``. This script calls that query directly. It does not use the
RailData API.

The trip planner is authoritative. When it returns a price, that price
replaces the PDF, including on a pair the PDF already lists. A blank or
``0.00`` planner field keeps the PDF amount for that field. A pair the
planner cannot price keeps the whole PDF row. Disagreements are still
written to ``data/fares/planner_mismatches.csv`` as an audit; that file
does not decide which amount is published.

Cache lives in ``data/fares/planner_cache/`` so a run can resume. Requests are
about one per second.
"""

from __future__ import annotations

import csv
import json
import sys
import time
import urllib.error
import urllib.request
import zipfile
from collections import defaultdict
from datetime import date
from decimal import Decimal, InvalidOperation
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "data" / "fares"
CACHE = OUT / "planner_cache"
GRAPHQL = "https://www.njtransit.com/api/graphql/graphql"
SOURCE_TITLE = "njtransit.com trip planner"
# Public trip-planner query the train-to fare card uses. Only the fare object
# is requested, not the trip list.
FARE_QUERY = """
query TrainSchedule($origin: String!, $destination: String!, $date: String) {
  getSchedule(origin: $origin, destination: $destination, date: $date) {
    fare {
      flexPass
      monthly
      reducedFare
      regularFare
      schoolMonthly
      tenTrip
      weekly
      showMessage
    }
  }
}
""".strip()
STATION_QUERY = """
query Index {
  getTrainScheduleStationsRailForSchedules { title stationCode }
}
""".strip()
COMPARE_FIELDS = (
    "adult_ow",
    "reduced_ow",
    "ten_trip",
    "weekly",
    "flexpass",
    "monthly",
    "school_monthly",
)
SECAUCUS_STOPS = {"38174", "38187"}
HOBOKEN_ROUTES = {"MNE", "MNEG", "BNTN", "BNTNM", "MNBN", "MNBNP", "PASC"}
NEWARK_ROUTES = {"NEC", "NJCL", "NJCLL", "RARV", "PRIN", "ATLC"}
# GTFS stop name (normalized) -> planner station title fragment, when the
# automatic name match is ambiguous or the planner uses a longer name.
NAME_ALIASES = {
    "30TH ST PHL": "30TH STREET PHILADELPHIA",
    "POINT PLEASANT": "POINT PLEASANT BEACH",
    "RAMSEY": "RAMSEY MAIN STREET",
    "MSU": "MONTCLAIR STATE UNIVERSITY",
    "JERSEY AVE": "JERSEY AVENUE",
    "BROADWAY": "BROADWAY FAIR LAWN",
    "ESSEX STREET": "ESSEX STREET PVL",
    "MIDDLETOWN NJ": "MIDDLETOWN NEW JERSEY",
    "MIDDLETOWN NY": "MIDDLETOWN NEW YORK",
    "HOHOKUS": "HO HO KUS",
    "PRINCETON JCT": "PRINCETON JUNCTION",
    "SECAUCUS LOWER LEVEL": "SECAUCUS JUNCTION",
    "SECAUCUS UPPER LEVEL": "SECAUCUS JUNCTION",
    "NEWARK AIRPORT RAILROAD": "NEWARK AIRPORT",
    "NEWARK BROAD ST": "NEWARK BROAD STREET",
    "NEWARK PENN": "NEWARK PENN",
    "NEW YORK PENN": "NEW YORK PENN",
    "WOOD RIDGE": "WOOD RIDGE",
    "WAYNE ROUTE 23 TRANSIT CENTER RR": "WAYNE ROUTE 23 TRANSIT CENTER",
    "SALISBURY MILLS CORNWALL": "SALISBURY MILLS CORNWALL",
}


def norm(text: str) -> str:
    cleaned = text.upper().replace("&", " AND ")
    for char in ".'’":
        cleaned = cleaned.replace(char, "")
    cleaned = cleaned.replace("-", " ").replace("/", " ")
    cleaned = cleaned.replace("(", " ").replace(")", " ").replace("[", " ").replace("]", " ")
    for word in ("STATION", "RAILROAD", "TERMINAL"):
        cleaned = cleaned.replace(f" {word}", " ")
    return " ".join(cleaned.split())


def money(value) -> str:
    if value is None:
        return ""
    text = str(value).replace("$", "").replace(",", "").strip()
    if not text:
        return ""
    try:
        return f"{Decimal(text):.2f}"
    except InvalidOperation:
        return ""


def post_graphql(query: str, variables: dict | None, timeout: float = 45) -> dict:
    payload = json.dumps({"query": query, "variables": variables or {}}).encode()
    request = urllib.request.Request(
        GRAPHQL,
        data=payload,
        headers={
            "content-type": "application/json",
            "accept": "application/json",
            "user-agent": "njt-gtfs-proxy-fare-refresh",
        },
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read().decode())


def load_planner_stations() -> dict[str, str]:
    """Return station code -> one planner title."""
    CACHE.mkdir(parents=True, exist_ok=True)
    path = CACHE / "stations.json"
    if path.exists():
        raw = json.loads(path.read_text())
    else:
        raw = post_graphql(STATION_QUERY, {})
        path.write_text(json.dumps(raw))
        time.sleep(1)
    by_code: dict[str, list[str]] = defaultdict(list)
    for row in raw["data"]["getTrainScheduleStationsRailForSchedules"]:
        code = row.get("stationCode")
        title = row.get("title") or ""
        if code is None or not title:
            continue
        by_code[str(code)].append(title)
    chosen = {}
    for code, titles in by_code.items():
        # Prefer the title that is not a duplicate nickname when several exist.
        titles = sorted(set(titles), key=len)
        chosen[code] = titles[0] if len(titles) == 1 else _prefer_title(titles)
    return chosen


def _prefer_title(titles: list[str]) -> str:
    preferred = (
        "New York Penn Station",
        "Newark Penn Station",
        "Newark Airport Rail Station",
        "Meadowlands Rail Station",
        "30th Street Station Philadelphia",
        "Montclair State University Station",
    )
    for title in preferred:
        if title in titles:
            return title
    return sorted(titles, key=len)[0]


def map_stops(rail_stops: list[dict], planner: dict[str, str]) -> dict[str, dict]:
    by_norm: dict[str, list[tuple[str, str]]] = defaultdict(list)
    for code, title in planner.items():
        by_norm[norm(title)].append((code, title))
    mapped = {}
    unmatched = []
    for stop in rail_stops:
        key = NAME_ALIASES.get(norm(stop["stop_name"]), norm(stop["stop_name"]))
        hits = by_norm.get(key, [])
        if len(hits) != 1:
            # Fall back to a unique planner title that contains every token.
            hits = _fuzzy(key, planner)
        if len(hits) != 1:
            unmatched.append((stop["stop_id"], stop["stop_name"], key, hits))
            continue
        code, title = hits[0]
        mapped[stop["stop_id"]] = {
            "stop_id": stop["stop_id"],
            "stop_name": stop["stop_name"],
            "routes": stop["routes"],
            "code": code,
            "title": title,
        }
    if unmatched:
        print("UNMATCHED GTFS STOPS", file=sys.stderr)
        for row in unmatched:
            print(f"  {row[0]} {row[1]} -> {row[2]} hits={row[3]}", file=sys.stderr)
        raise SystemExit(f"{len(unmatched)} rail stops did not match a planner station")
    return mapped


def _fuzzy(key: str, planner: dict[str, str]) -> list[tuple[str, str]]:
    tokens = set(key.split())
    found = []
    for code, title in planner.items():
        title_tokens = set(norm(title).split())
        if tokens and tokens <= title_tokens:
            found.append((code, title))
    # If several codes share a title family, keep unique codes only.
    codes = {code for code, _title in found}
    if len(codes) == 1:
        return [found[0]]
    return found


def same_line_pairs(gtfs_zip: Path) -> set[tuple[str, str]]:
    pairs: set[tuple[str, str]] = set()
    with zipfile.ZipFile(gtfs_zip) as archive:
        routes = {
            row["route_id"]: row
            for row in csv.DictReader(archive.read("routes.txt").decode().splitlines())
        }
        rail = {route_id for route_id, row in routes.items() if row["route_type"] == "2"}
        trips = {
            row["trip_id"]: row["route_id"]
            for row in csv.DictReader(archive.read("trips.txt").decode().splitlines())
            if row["route_id"] in rail
        }
        by_trip: dict[str, set[str]] = defaultdict(set)
        for row in csv.DictReader(archive.read("stop_times.txt").decode().splitlines()):
            if row["trip_id"] in trips:
                by_trip[row["trip_id"]].add(row["stop_id"])
    for stops in by_trip.values():
        ordered = sorted(stops)
        for index, origin in enumerate(ordered):
            for dest in ordered[index + 1 :]:
                pairs.add((origin, dest))
    return pairs


def division(routes: str) -> str:
    tokens = set(routes.split())
    hoboken = bool(tokens & HOBOKEN_ROUTES)
    newark = bool(tokens & NEWARK_ROUTES)
    if hoboken and not newark:
        return "hoboken"
    if newark and not hoboken:
        return "newark"
    return "both"


def extra_pairs(mapped: dict[str, dict], already: set[tuple[str, str]]) -> set[tuple[str, str]]:
    """Hoboken↔Newark-division and Newark Penn↔Hoboken-division pairs with no shared trip."""
    hoboken = "63"
    newark = "107"
    extra = set()
    for stop_id, stop in mapped.items():
        side = division(stop["routes"])
        if side == "newark" and stop_id != hoboken:
            pair = tuple(sorted((hoboken, stop_id)))
            if pair not in already and pair[0] != pair[1]:
                extra.add(pair)
        if side == "hoboken" and stop_id != newark:
            pair = tuple(sorted((newark, stop_id)))
            if pair not in already and pair[0] != pair[1]:
                extra.add(pair)
    return extra


def area_for(stop_id: str, stop_name: str) -> tuple[str, str]:
    if stop_id in SECAUCUS_STOPS:
        return "njt_secaucus", "Secaucus Junction"
    return f"njt_stop_{stop_id}", stop_name


def cache_path(code_a: str, code_b: str) -> Path:
    left, right = sorted((code_a, code_b))
    return CACHE / f"{left}_{right}.json"


def fetch_fare(origin_title: str, dest_title: str, when: str) -> dict:
    body = post_graphql(
        FARE_QUERY,
        {"origin": origin_title, "destination": dest_title, "date": when},
    )
    if body.get("errors"):
        message = body["errors"][0].get("message", "")
        # A holiday with no trips returns this instead of a fare. Ask again
        # for a normal Wednesday; the fare itself does not depend on the date.
        if when != "12/02/2026" and "unable to provide trip results" in message:
            time.sleep(1)
            return fetch_fare(origin_title, dest_title, "12/02/2026")
        raise RuntimeError(message[:300])
    fare = (body.get("data") or {}).get("getSchedule", {}).get("fare")
    if not fare or not fare.get("regularFare"):
        if when != "12/02/2026":
            time.sleep(1)
            return fetch_fare(origin_title, dest_title, "12/02/2026")
        return {}
    return {
        "adult_ow": money(fare.get("regularFare")),
        "reduced_ow": money(fare.get("reducedFare")),
        "ten_trip": money(fare.get("tenTrip")),
        "weekly": money(fare.get("weekly")),
        "flexpass": money(fare.get("flexPass")),
        "monthly": money(fare.get("monthly")),
        "school_monthly": money(fare.get("schoolMonthly")),
        "show_message": bool(fare.get("showMessage")),
        "queried_date": when,
    }


def load_or_fetch(code_a: str, title_a: str, code_b: str, title_b: str, when: str) -> dict:
    path = cache_path(code_a, code_b)
    if path.exists():
        return json.loads(path.read_text())
    fare: dict = {}
    for attempt in range(3):
        try:
            fare = fetch_fare(title_a, title_b, when)
            break
        except (urllib.error.URLError, TimeoutError, RuntimeError, json.JSONDecodeError, OSError) as err:
            print(f"  retry {attempt + 1} {code_a}-{code_b}: {err}", file=sys.stderr, flush=True)
            time.sleep(2)
    path.write_text(json.dumps(fare))
    time.sleep(1)
    return fare


def read_csv(path: Path) -> list[dict]:
    with path.open() as handle:
        return list(csv.DictReader(handle))


def write_csv(path: Path, fields: list[str], rows: list[dict]) -> None:
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fields, lineterminator="\n", extrasaction="ignore")
        writer.writeheader()
        writer.writerows(rows)


def expand_pdf(fares: list[dict], stations: list[dict]) -> dict[tuple[str, str], dict]:
    by_area = defaultdict(list)
    for station in stations:
        by_area[station["area_id"]].append(station)
    expanded = {}
    for fare in fares:
        origins = by_area.get(fare["origin_area_id"], [])
        dests = by_area.get(fare["destination_area_id"], [])
        for origin in origins:
            for dest in dests:
                origin_area, origin_name = area_for(origin["stop_id"], origin["stop_name"])
                dest_area, dest_name = area_for(dest["stop_id"], dest["stop_name"])
                if origin_area == dest_area:
                    continue
                key = tuple(sorted((origin_area, dest_area)))
                row = dict(fare)
                row["origin_area_id"], row["destination_area_id"] = key
                names = {origin_area: origin_name, dest_area: dest_name}
                row["origin_name"] = names[key[0]]
                row["destination_name"] = names[key[1]]
                expanded[key] = row
    return expanded


def amounts_differ(pdf_row: dict, planner: dict) -> list[tuple[str, str, str]]:
    diffs = []
    for field in COMPARE_FIELDS:
        pdf_value = pdf_row.get(field, "") or ""
        planner_value = planner.get(field, "") or ""
        if pdf_value == planner_value:
            continue
        if pdf_value and planner_value and Decimal(pdf_value) == Decimal(planner_value):
            continue
        diffs.append((field, pdf_value, planner_value))
    return diffs


def _blank_zero(value: str) -> str:
    """A planner 0.00 pass is an unsold product, not a free fare."""
    if value in {"0.00", "0"}:
        return ""
    return value


def blank_amounts() -> dict:
    return {field: "" for field in ("adult_ow", "reduced_ow", "adult_rt", "child_rt", *COMPARE_FIELDS[2:])}


def overlay_planner(pdf_row: dict, fare: dict, fetched: str) -> tuple[dict, int]:
    """Copy planner prices onto a PDF row.

    A planner field that is blank or ``0.00`` leaves the PDF amount in place.
    Round-trip columns stay on the PDF row because the fare card has no
    round-trip price. The row's source becomes the trip planner, since that
    is the authority for every field it did price.

    Returns the merged row and how many amount fields changed.
    """
    row = dict(pdf_row)
    changed = 0
    for field in COMPARE_FIELDS:
        planner_value = _blank_zero(fare.get(field, "") or "")
        if not planner_value:
            continue
        if (row.get(field) or "") != planner_value:
            changed += 1
        row[field] = planner_value
    row["source_file"] = GRAPHQL
    row["source_title"] = SOURCE_TITLE
    row["source_status"] = "live"
    row["source_date"] = fetched
    row["source_sha256"] = ""
    return row, changed


def planner_row(area_a: str, name_a: str, area_b: str, name_b: str, fare: dict, fetched: str) -> dict:
    left, right = ((area_a, name_a), (area_b, name_b)) if area_a <= area_b else ((area_b, name_b), (area_a, name_a))
    row = blank_amounts()
    row.update(
        {
            "origin_area_id": left[0],
            "destination_area_id": right[0],
            "origin_name": left[1],
            "destination_name": right[1],
            "zone": "",
            "is_exception": "0",
            "adult_ow": _blank_zero(fare.get("adult_ow", "")),
            "reduced_ow": _blank_zero(fare.get("reduced_ow", "")),
            "ten_trip": _blank_zero(fare.get("ten_trip", "")),
            "weekly": _blank_zero(fare.get("weekly", "")),
            "flexpass": _blank_zero(fare.get("flexpass", "")),
            "monthly": _blank_zero(fare.get("monthly", "")),
            "school_monthly": _blank_zero(fare.get("school_monthly", "")),
            "source_file": GRAPHQL,
            "source_title": SOURCE_TITLE,
            "source_status": "live",
            "source_date": fetched,
            "source_sha256": "",
        }
    )
    return row


def run(gtfs_zip: Path, limit: int = 0, dry_run: bool = False) -> None:
    fetched_on = date.today().isoformat()
    planner = load_planner_stations()
    rail_stops = read_csv(OUT / "rail_stops.csv")
    mapped = map_stops(rail_stops, planner)
    line_pairs = same_line_pairs(gtfs_zip)
    # Drop pairs whose stops are not in the rail directory (should not happen).
    line_pairs = {pair for pair in line_pairs if pair[0] in mapped and pair[1] in mapped}
    cross = extra_pairs(mapped, line_pairs)
    pdf_fares = read_csv(OUT / "fares.csv")
    pdf_stations = read_csv(OUT / "stations.csv")
    pdf_expanded = expand_pdf(pdf_fares, pdf_stations)
    # PDF pairs also need a planner check, even when no single trip serves both.
    pdf_stop_pairs = set()
    by_area = defaultdict(list)
    for station in pdf_stations:
        by_area[station["area_id"]].append(station["stop_id"])
    for fare in pdf_fares:
        for origin in by_area.get(fare["origin_area_id"], []):
            for dest in by_area.get(fare["destination_area_id"], []):
                if origin != dest:
                    pdf_stop_pairs.add(tuple(sorted((origin, dest))))
    wanted = set()
    for origin, dest in line_pairs | cross | pdf_stop_pairs:
        code_a = mapped[origin]["code"]
        code_b = mapped[dest]["code"]
        if code_a == code_b:
            continue
        wanted.add((origin, dest, code_a, code_b))
    # One network call per station-code pair.
    by_codes: dict[tuple[str, str], tuple[str, str, str, str]] = {}
    for origin, dest, code_a, code_b in wanted:
        key = tuple(sorted((code_a, code_b)))
        by_codes.setdefault(
            key,
            (mapped[origin]["title"] if mapped[origin]["code"] == key[0] else mapped[dest]["title"],
             mapped[origin]["title"] if mapped[origin]["code"] == key[1] else mapped[dest]["title"],
             key[0],
             key[1]),
        )
    # The tuple construction above is brittle if both stops aren't origin/dest aligned.
    queries = []
    for (code_left, code_right) in sorted(by_codes):
        title_left = next(stop["title"] for stop in mapped.values() if stop["code"] == code_left)
        title_right = next(stop["title"] for stop in mapped.values() if stop["code"] == code_right)
        queries.append((code_left, title_left, code_right, title_right))
    print(
        f"same-line pairs={len(line_pairs)} cross-division extras={len(cross)} "
        f"pdf pairs={len(pdf_stop_pairs)} planner queries={len(queries)}"
    )
    if dry_run:
        return
    if limit:
        queries = queries[:limit]
    when = "11/26/2026"
    fares_by_code = {}
    for index, (code_a, title_a, code_b, title_b) in enumerate(queries, start=1):
        fare = load_or_fetch(code_a, title_a, code_b, title_b, when)
        fares_by_code[(code_a, code_b)] = fare
        if index % 50 == 0 or index == len(queries):
            print(f"fetched {index}/{len(queries)}", flush=True)

    mismatches = []
    merged = dict(pdf_expanded)
    planner_added = 0
    checked = 0
    fields_changed = 0
    for origin, dest, code_a, code_b in wanted:
        key = tuple(sorted((code_a, code_b)))
        fare = fares_by_code.get(key) or {}
        if not fare.get("adult_ow"):
            continue
        origin_area, origin_name = area_for(origin, mapped[origin]["stop_name"])
        dest_area, dest_name = area_for(dest, mapped[dest]["stop_name"])
        area_key = tuple(sorted((origin_area, dest_area)))
        if area_key in pdf_expanded:
            checked += 1
            for field, pdf_value, planner_value in amounts_differ(pdf_expanded[area_key], fare):
                mismatches.append(
                    {
                        "origin_stop_id": origin,
                        "destination_stop_id": dest,
                        "origin_name": mapped[origin]["stop_name"],
                        "destination_name": mapped[dest]["stop_name"],
                        "field": field,
                        "pdf_value": pdf_value,
                        "planner_value": planner_value,
                        "pdf_source_status": pdf_expanded[area_key]["source_status"],
                        "pdf_source_file": pdf_expanded[area_key]["source_file"],
                    }
                )
            merged[area_key], changed = overlay_planner(pdf_expanded[area_key], fare, fetched_on)
            fields_changed += changed
            continue
        if area_key in merged and merged[area_key]["source_title"] == SOURCE_TITLE:
            continue
        merged[area_key] = planner_row(
            origin_area, origin_name, dest_area, dest_name, fare, fetched_on
        )
        planner_added += 1

    rows = sorted(merged.values(), key=lambda row: (row["origin_area_id"], row["destination_area_id"], row["adult_ow"]))
    fields = list(pdf_fares[0].keys())
    write_csv(OUT / "fares.csv", fields, rows)

    station_rows = []
    seen_areas = set()
    for stop in sorted(mapped.values(), key=lambda item: int(item["stop_id"])):
        area_id, area_name = area_for(stop["stop_id"], stop["stop_name"])
        if area_id in seen_areas and stop["stop_id"] not in SECAUCUS_STOPS:
            continue
        # Both Secaucus platforms are listed; the area id repeats on purpose.
        station_rows.append(
            {
                "stop_id": stop["stop_id"],
                "stop_name": stop["stop_name"],
                "area_id": area_id,
                "area_name": area_name,
                "zone": "",
                "routes": stop["routes"],
                "chart_names": stop["title"],
            }
        )
        seen_areas.add(area_id)
    write_csv(
        OUT / "stations.csv",
        ["stop_id", "stop_name", "area_id", "area_name", "zone", "routes", "chart_names"],
        station_rows,
    )
    write_csv(
        OUT / "planner_mismatches.csv",
        [
            "origin_stop_id",
            "destination_stop_id",
            "origin_name",
            "destination_name",
            "field",
            "pdf_value",
            "planner_value",
            "pdf_source_status",
            "pdf_source_file",
        ],
        mismatches,
    )
    priced_areas = {row["origin_area_id"] for row in rows} | {row["destination_area_id"] for row in rows}
    gaps = []
    for stop in station_rows:
        if stop["area_id"] not in priced_areas:
            gaps.append(
                {
                    "stop_id": stop["stop_id"],
                    "stop_name": stop["stop_name"],
                    "routes": stop["routes"],
                    "reason": "trip_planner_returned_no_fare",
                }
            )
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
            "reason": "printed_in_zone_19_but_absent_from_rail_gtfs_and_not_a_planner_pair",
        }
    )
    # Monmouth Park is a planner station. If it is not in GTFS it cannot be a stop area.
    gaps.sort(key=lambda row: (row["reason"], row["stop_name"]))
    write_csv(OUT / "gaps.csv", ["stop_id", "stop_name", "routes", "reason"], gaps)

    by_status = defaultdict(int)
    pair_mismatches = set()
    for row in mismatches:
        by_status[row["pdf_source_status"]] += 1
        pair_mismatches.add((row["origin_stop_id"], row["destination_stop_id"], row["pdf_source_status"]))
    print(
        f"planner_rows_added={planner_added} pdf_pairs_checked={checked} "
        f"planner_fields_changed={fields_changed} "
        f"mismatch_fields={len(mismatches)} mismatch_pairs={len(pair_mismatches)} "
        f"by_pdf_status={dict(by_status)} fare_rows={len(rows)} gaps={len(gaps)}"
    )


def main() -> int:
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gtfs", type=Path, required=True, help="Public NJT rail GTFS zip")
    parser.add_argument("--limit", type=int, default=0, help="Fetch at most this many code pairs")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    run(args.gtfs, limit=args.limit, dry_run=args.dry_run)
    return 0


if __name__ == "__main__":
    sys.exit(main())
