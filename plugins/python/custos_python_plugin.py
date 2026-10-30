#!/usr/bin/env python3
"""custos Python plugin loader (docs/guide/plugins.md §4.3 contract).

Loads one user script and serves it over the custos/v1 JSON-RPC stdio
protocol. The user script provides:

    def probe(params: dict) -> dict
        {"healthy": bool, "observation": str}
    def recover(params: dict) -> dict            (optional)
        {"succeeded": bool, "detail": str}
    def verify(params: dict) -> dict             (optional)
    CAPABILITIES = [ ... ]                        (optional override)

stdout discipline: the loader owns stdout — anything the user script
prints is redirected to stderr BEFORE the script is imported, so a
chatty script can never corrupt the protocol (§4.4). The timeout,
restart policy and permission gates are enforced by the host
(custos-plugin-host), not here: same口径 as native plugins.

Usage: python3 custos_python_plugin.py <script.py>
"""

import importlib.util
import json
import sys

PROTOCOL_VERSION = "1"
METHOD_PREFIX = "custos/v1/"

_real_stdout = sys.stdout
sys.stdout = sys.stderr  # protocol owns stdout from here on

if len(sys.argv) != 2:
    print(f"usage: {sys.argv[0]} <script.py>", file=sys.stderr)
    sys.exit(2)

script_path = sys.argv[1]
spec = importlib.util.spec_from_file_location("custos_user_script", script_path)
if spec is None or spec.loader is None:
    print(f"cannot load script: {script_path}", file=sys.stderr)
    sys.exit(2)
module = importlib.util.module_from_spec(spec)
try:
    spec.loader.exec_module(module)
except Exception as exc:  # noqa: BLE001 - report, never crash the protocol
    print(f"script import failed: {exc}", file=sys.stderr)
    sys.exit(2)


def _as_dict(value):
    """Normalizes a user return value to a plain dict."""
    if isinstance(value, dict):
        return value
    if isinstance(value, (tuple, list)) and len(value) == 2:
        # (healthy/flag, text) shorthand
        return {"healthy": bool(value[0]), "observation": str(value[1])}
    raise TypeError(f"expected dict or (flag, text) pair, got {type(value).__name__}")


def _capabilities():
    override = getattr(module, "CAPABILITIES", None)
    if isinstance(override, list) and override:
        return override
    caps = []
    if hasattr(module, "probe"):
        caps.append(
            {
                "kind": "probe_provider",
                "operation_id": "python.probe",
                "summary": f"script probe: {script_path}",
                "read_only": True,
                "required_permission": "probe.read",
                "affected_scope": "named_resource",
                "interrupts_network": False,
                "interrupts_remote_admin": False,
                "cancelable": False,
                "idempotent": True,
                "verifiable": True,
                "possible_failures": ["script_error"],
            }
        )
    if hasattr(module, "recover"):
        caps.append(
            {
                "kind": "recovery_action_provider",
                "operation_id": "python.recover",
                "summary": f"script recovery: {script_path}",
                "read_only": False,
                "required_permission": "script.recover",
                "affected_scope": "named_resource",
                "interrupts_network": False,
                "interrupts_remote_admin": False,
                "cancelable": False,
                "idempotent": False,
                "verifiable": True,
                "possible_failures": ["script_error"],
            }
        )
    return caps


def _call(name, params):
    fn = getattr(module, name, None)
    if fn is None:
        raise LookupError(f"script defines no {name}()")
    result = fn(dict(params) if isinstance(params, dict) else {})
    return _as_dict(result)


def _reply(rid, result):
    _real_stdout.write(
        json.dumps({"jsonrpc": "2.0", "id": rid, "result": result}, separators=(",", ":")) + "\n"
    )
    _real_stdout.flush()


def _reply_error(rid, code, message):
    _real_stdout.write(
        json.dumps(
            {"jsonrpc": "2.0", "id": rid, "error": {"code": code, "message": message}},
            separators=(",", ":"),
        )
        + "\n"
    )
    _real_stdout.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except ValueError:
            continue  # undecodable line: skip, never crash (§4.4)
        rid = request.get("id")
        method = request.get("method", "")
        params = request.get("params") or {}

        if method == METHOD_PREFIX + "initialize":
            _reply(
                rid,
                {
                    "protocol_version": PROTOCOL_VERSION,
                    "plugin": f"custos-python-loader: {script_path}",
                },
            )
        elif method == METHOD_PREFIX + "describe_capabilities":
            _reply(rid, _capabilities())
        elif method == METHOD_PREFIX + "probe":
            try:
                result = _call("probe", params)
                if "healthy" not in result:
                    raise ValueError("probe result missing 'healthy'")
                _reply(rid, result)
            except LookupError as exc:
                _reply_error(rid, -32601, str(exc))
            except Exception as exc:  # noqa: BLE001
                _reply_error(rid, -32000, f"script error: {exc}")
        elif method == METHOD_PREFIX + "execute_recovery_action":
            try:
                result = _call("recover", params)
                if "succeeded" not in result:
                    raise ValueError("recover result missing 'succeeded'")
                _reply(rid, result)
            except LookupError as exc:
                _reply_error(rid, -32601, str(exc))
            except Exception as exc:  # noqa: BLE001
                _reply_error(rid, -32000, f"script error: {exc}")
        elif method == METHOD_PREFIX + "verify":
            try:
                if hasattr(module, "verify"):
                    _reply(rid, _call("verify", params))
                else:
                    _reply(
                        None,
                        rid,
                        {"healthy": None, "observation": "script defines no verify()"},
                    )
            except Exception as exc:  # noqa: BLE001
                _reply_error(rid, -32000, f"script error: {exc}")
        elif method == METHOD_PREFIX + "list_recovery_actions":
            _reply(
                rid,
                [c for c in _capabilities() if c.get("kind") == "recovery_action_provider"],
            )
        elif method == METHOD_PREFIX + "shutdown":
            _reply(rid, {"ok": True})
            return 0
        else:
            _reply_error(rid, -32601, f"unknown method: {method}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
