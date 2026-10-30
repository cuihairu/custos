#!/usr/bin/env python3
"""Outbound sample: trigger one probe -> diagnose -> recover -> notify pass.

Same engine the CLI runs (custos_core::assemble): budgets, mutexes,
dependency ordering and independent verification all apply. Python cannot
bypass any of it (ADR-0005, ADR-0015).

    python3 outbound_trigger.py custos.toml custos.db
"""

import sys

import custos


def main() -> int:
    if len(sys.argv) < 3:
        print("usage: outbound_trigger.py <CONFIG> <DB>", file=sys.stderr)
        return 2
    config_path, db_path = sys.argv[1], sys.argv[2]

    result = custos.run_once(config_path, db_path)

    print("finished runs:")
    for run in result["runs"]:
        print(f"  {run['run_id']} {run['resource_id']} -> {run['status']}")

    print("health after the pass:")
    for row in result["health"]:
        state = "OK " if row["succeeded"] else "BAD"
        print(f"  [{state}] {row['resource_id']}: {row['observation']}")

    notes = result["notifications"]
    print(f"notifications: delivered={notes['delivered']} retried={notes['retried']} "
          f"dead_letter={notes['dead_letter']} queue_total={notes['queue']['total']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
