# custos Python plugin loader

Run probe/recovery logic as Python scripts under the same plugin contract
as native plugins: JSON-RPC over stdio (`custos/v1/*`), timeout, restart
policy, permission gates and budgets are enforced by `custos-plugin-host`
— identical口径 (ADR-0002, ADR-0015).

## Writing a script

A script defines any of:

```python
def probe(params: dict) -> dict: ...   # {"healthy": bool, "observation": str}
def recover(params: dict) -> dict: ... # {"succeeded": bool, "detail": str}
def verify(params: dict) -> dict: ...  # optional
CAPABILITIES = [...]                   # optional capability override
```

`params` is the operation's parameter object (`resource_id`, `args`, ...).
Capabilities default to `python.probe` / `python.recover` derived from
which functions exist. `print()` is redirected to stderr automatically:
scripts may log freely; the loader owns stdout (§4.4).

See `examples/file_health_probe.py` for a complete probe + recovery pair.

## Running

The loader ships the shim; the Rust side (`custos-plugin-host::python`)
spawns `python3 custos_python_plugin.py <script.py>` (interpreter
overridable via `CUSTOS_PYTHON_INTERPRETER`). `PluginProbe`/supervisor
wrap it exactly like a native plugin process.
