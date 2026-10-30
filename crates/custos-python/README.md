# custos-python

Python bindings for Custos (todo.md P1: Python 出向, ADR-0015). Query the
engine's SQLite state and trigger one probe/recovery pass from Python —
through the real engine assembly (`custos_core::assemble`), so budgets,
per-resource mutexes, dependency ordering and verification all apply.
Python sees tool calls, never arbitrary execution.

## Build

```bash
pip install maturin
maturin build --release --features extension-module
pip install <wheel>
```

The crate pins `abi3-py39`: one wheel works on Python 3.9+.

## API

```python
import custos

custos.version()

# Query handle over the engine's database (read-only):
h = custos.open("custos.db")
h.health()          # latest probe verdict per resource
h.probes(limit=50)  # recent evidence, newest first
h.runs(limit=20)    # recovery runs
h.incidents(limit=20)
h.audit(limit=50)
h.notifications()   # queue counts (five states) + entries

# Trigger one full pass (probe -> diagnose -> recover -> notify):
custos.run_once("custos.toml", "custos.db")
# -> {"runs": [...], "health": [...], "notifications": {...}}
```

Examples live in `plugins/python/examples/` (`outbound_query.py`,
`outbound_trigger.py`); the inbound script loader is documented in
`plugins/python/README.md`.
