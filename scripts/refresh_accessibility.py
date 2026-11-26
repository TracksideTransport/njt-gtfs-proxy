#!/usr/bin/env python3
"""Refresh rail station accessibility from the public NJ Transit GraphQL API.

One request reads ``getTrainScheduleStationsRailForDV`` (``pentaStationID``
and ``accessible``) and ``getTrainScheduleStationsRailForSchedules`` (the
trip-planner station code). ``pentaStationID`` is the developer rail GTFS
``stop_code``. The proxy writes ``wheelchair_boarding`` 1 or 2 from
``data/accessibility/rail_stations.csv``.

This does not use RailData credentials.
"""

from __future__ import annotations

import csv
import json
import urllib.request
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "data" / "accessibility" / "rail_stations.csv"
GRAPHQL = "https://www.njtransit.com/api/graphql/graphql"
QUERY = """
query Index {
  getTrainScheduleStationsRailForDV { title pentaStationID accessible }
  getTrainScheduleStationsRailForSchedules { title stationCode accessible }
}
""".strip()


def main() -> int:
    """Fetch the station lists and rewrite the CSV."""
    body = json.dumps({"operationName": "Index", "query": QUERY}).encode()
    request = urllib.request.Request(
        GRAPHQL,
        data=body,
        headers={
            "content-type": "application/json",
            "accept": "application/json",
            "user-agent": "njt-gtfs-proxy-fare-refresh",
        },
    )
    with urllib.request.urlopen(request, timeout=45) as response:
        payload = json.loads(response.read().decode())
    data = payload["data"]
    schedules = {row["title"]: row for row in data["getTrainScheduleStationsRailForSchedules"]}
    grouped: dict[str, list[dict]] = defaultdict(list)
    for row in data["getTrainScheduleStationsRailForDV"]:
        code = (row.get("pentaStationID") or "").strip()
        if code:
            grouped[code].append(row)
    rows = []
    for code, items in sorted(grouped.items()):
        accessible = any(item.get("accessible") for item in items)
        title = items[0]["title"]
        station_code = ""
        for item in items:
            match = schedules.get(item["title"])
            if match and match.get("stationCode") is not None:
                title = item["title"]
                station_code = str(match["stationCode"])
                break
        rows.append(
            {
                "stop_code": code,
                "title": title,
                "station_code": station_code,
                "accessible": "1" if accessible else "0",
            }
        )
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with OUT.open("w", newline="") as handle:
        writer = csv.DictWriter(
            handle,
            fieldnames=["stop_code", "title", "station_code", "accessible"],
            lineterminator="\n",
        )
        writer.writeheader()
        writer.writerows(rows)
    accessible_count = sum(row["accessible"] == "1" for row in rows)
    print(f"stop_codes={len(rows)} accessible={accessible_count} not_accessible={len(rows) - accessible_count}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
