#!/usr/bin/env python3
"""Outbound sample: query a running engine's state from Python.

Requires the `custos` wheel (see crates/custos-python/README.md):
    maturin build --release --features extension-module && pip install <wheel>

Point it at the database the engine writes:
    python3 outbound_query.py /path/to/custos.db
"""

import sys

import custos


def main() -> int:
    db = sys.argv[1] if len(sys.argv) > 1 else "custos.db"
    print(custos.version())

    handle = custos.open(db)

    health = handle.health()
    print(f"\nhealth ({len(health)} resource(s)):")
    for row in health:
        state = "OK " if row["succeeded"] else "BAD"
        print(f"  [{state}] {row['resource_id']}: {row['observation']}")

    runs = handle.runs(5)
    print(f"\nrecent runs ({len(runs)}):")
    for run in runs:
        print(f"  {run['run_id']} {run['resource_id']} -> {run['status']}")

    notifications = handle.notifications()
    print(f"\nnotification queue: {notifications['counts']}")

    incidents = handle.incidents(5)
    print(f"incidents ({len(incidents)})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
