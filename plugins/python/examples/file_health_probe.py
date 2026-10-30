#!/usr/bin/env python3
"""Example custos Python probe + recovery script (plugins/python/README).

Probe rule: the target file's mtime age must stay under `max_age_seconds`
(default 120). Recovery: touch the file. This is the minimal but complete
script surface: probe() + recover(), both taking the operation params.

Note: print() here goes to stderr (the loader owns stdout), so scripts may
log freely without corrupting the protocol.
"""

import os
import time

TARGET = os.environ.get("EXAMPLE_HEALTH_TARGET", "/tmp/custos-python-example.target")
MAX_AGE_SECONDS = float(os.environ.get("EXAMPLE_HEALTH_MAX_AGE", "120"))


def probe(params):
    target = params.get("args", {}).get("target", TARGET)
    try:
        age = time.time() - os.path.getmtime(target)
    except OSError as exc:
        return {"healthy": False, "observation": f"cannot stat {target}: {exc}"}
    if age > MAX_AGE_SECONDS:
        return {"healthy": False, "observation": f"{target} is {age:.0f}s old (max {MAX_AGE_SECONDS:.0f}s)"}
    return {"healthy": True, "observation": f"{target} refreshed {age:.0f}s ago"}


def recover(params):
    target = params.get("args", {}).get("target", TARGET)
    try:
        with open(target, "a"):
            os.utime(target, None)
        return {"succeeded": True, "detail": f"touched {target}"}
    except OSError as exc:
        return {"succeeded": False, "detail": f"cannot touch {target}: {exc}"}
