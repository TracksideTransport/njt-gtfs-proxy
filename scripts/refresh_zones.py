#!/usr/bin/env python3
"""Rebuild ``data/fares/zones.csv`` from the checked-in fare tables.

NJ Transit prints a zone number beside each station on the New York Penn,
Hoboken, and Newark Penn fare charts. A zone is relative to the terminal: the
same station can be zone 7 from New York and zone 5 from Hoboken. This script
reads the zone that ``scripts/refresh_fares.py`` kept on each terminal pair in
``data/fares/fares.csv`` and writes one row per stop and terminal.

A pair between two terminals is skipped because the source chart is not
recorded once the trip planner has overlaid the row.

It reads local files only. It does not use credentials or the network.

Usage:
    python3 scripts/refresh_zones.py
"""

from __future__ import annotations

import csv
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FARES = ROOT / "data" / "fares"

# Terminal stop_id -> (terminal key, display name). The key is part of the
# area id the proxy writes (``njt_zone_{key}_{zone}``).
TERMINALS = {
    "105": ("ny", "New York Penn Station"),
    "63": ("hoboken", "Hoboken Terminal"),
    "107": ("newark", "Newark Penn Station"),
}


def read_csv(path: Path) -> list[dict]:
    """Read a CSV file into dict rows."""
    with path.open(newline="") as handle:
        return list(csv.DictReader(handle))


def main() -> int:
    """Derive per-terminal zones and rewrite ``zones.csv``."""
    stations = read_csv(FARES / "stations.csv")
    stops_by_area: dict[str, list[dict]] = defaultdict(list)
    for station in stations:
        stops_by_area[station["area_id"]].append(station)

    zones: dict[tuple[str, str], set[str]] = defaultdict(set)
    names: dict[str, str] = {}
    for fare in read_csv(FARES / "fares.csv"):
        zone = fare["zone"].strip()
        if not zone:
            continue
        sides = (fare["origin_area_id"], fare["destination_area_id"])
        terminal_of = {
            area: [TERMINALS[s["stop_id"]][0] for s in stops_by_area[area] if s["stop_id"] in TERMINALS]
            for area in sides
        }
        origin, destination = sides
        if terminal_of[origin] and not terminal_of[destination]:
            terminal, other = terminal_of[origin][0], destination
        elif terminal_of[destination] and not terminal_of[origin]:
            terminal, other = terminal_of[destination][0], origin
        else:
            continue
        for stop in stops_by_area[other]:
            zones[(stop["stop_id"], terminal)].add(zone)
            names[stop["stop_id"]] = stop["stop_name"]

    conflicts = {key: sorted(values) for key, values in zones.items() if len(values) > 1}
    if conflicts:
        for (stop_id, terminal), values in sorted(conflicts.items()):
            print(f"conflict: stop {stop_id} from {terminal}: zones {values}", file=sys.stderr)
        raise SystemExit(f"{len(conflicts)} stop/terminal pairs have more than one zone")

    order = {key: index for index, (key, _name) in enumerate(TERMINALS.values())}
    rows = [
        {
            "stop_id": stop_id,
            "stop_name": names[stop_id],
            "terminal": terminal,
            "zone": next(iter(values)),
        }
        for (stop_id, terminal), values in zones.items()
    ]
    rows.sort(key=lambda row: (order[row["terminal"]], int(row["zone"]), int(row["stop_id"])))
    with (FARES / "zones.csv").open("w", newline="") as handle:
        writer = csv.DictWriter(
            handle, fieldnames=["stop_id", "stop_name", "terminal", "zone"], lineterminator="\n"
        )
        writer.writeheader()
        writer.writerows(rows)
    print(f"wrote {len(rows)} rows to {FARES / 'zones.csv'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
