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


def resolve_target(params):
    """Resolve `args.target` to a path inside the example's base directory.

    A recovery plan is config data, not a shell: a raw `target` let a plan
    probe or touch any file the process can reach. Only bare relative
    names are accepted, resolved against the base directory (the
    env-configured default's parent); absolute paths and `..` escapes are
    refused. The env default itself stays the operator's choice (S1).
    """
    raw = params.get("args", {}).get("target")
    if raw is None:
        return TARGET
    if not isinstance(raw, str) or not raw.strip():
        raise ValueError("target must be a non-empty string")
    if os.path.isabs(raw):
        raise ValueError(f"target must be a relative filename, got {raw!r}")
    normalized = os.path.normpath(raw)
    if normalized in ("", ".") or normalized == ".." or normalized.startswith("../"):
        raise ValueError(f"target must not escape the base directory: {raw!r}")
    return os.path.join(os.path.dirname(TARGET) or ".", normalized)


def probe(params):
    try:
        target = resolve_target(params)
    except ValueError as exc:
        return {"healthy": False, "observation": f"refused target: {exc}"}
    try:
        age = time.time() - os.path.getmtime(target)
    except OSError as exc:
        return {"healthy": False, "observation": f"cannot stat {target}: {exc}"}
    if age > MAX_AGE_SECONDS:
        return {"healthy": False, "observation": f"{target} is {age:.0f}s old (max {MAX_AGE_SECONDS:.0f}s)"}
    return {"healthy": True, "observation": f"{target} refreshed {age:.0f}s ago"}


def recover(params):
    try:
        target = resolve_target(params)
    except ValueError as exc:
        return {"succeeded": False, "detail": f"refused target: {exc}"}
    try:
        with open(target, "a"):
            os.utime(target, None)
        return {"succeeded": True, "detail": f"touched {target}"}
    except OSError as exc:
        return {"succeeded": False, "detail": f"cannot touch {target}: {exc}"}
