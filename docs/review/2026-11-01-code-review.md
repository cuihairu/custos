# Code Review — 2026-11-01

Full-repo review across the `custos` workspace: static security scan, hidden-bug
hunt (resource / panic / boundary / concurrency / error handling), and the
quality gate (fmt + clippy + `cargo test --workspace`).

Method: three read-only scans (security, resource/panic/boundary, concurrency/
error-handling) produced a findings queue; each finding was verified against the
current source at `file:line` before fixing or recording. Every fix is a single
issue, one commit, with the workspace test suite green at the time of commit.

## Summary

| Severity | Found | Fixed | Recorded (todo) |
|----------|------:|------:|----------------:|
| P0       | 1     | 1     | 0 |
| P1       | 15    | 15    | 0 |
| P2       | 28    | 0     | 28 |
| P3       | 5     | 0     | 5 |
| **Total**| **49**| **16**| **33** |

- **Fixed (16)**: R1–R8 (R8 covers both the plugin protocol and the host stderr
  reader, fixed in two commits), C1, C3, C10, C11, C12, C25, C26, C28 — across
  13 review commits.
- **Recorded (33)**: P2/P3 items needing a design decision or a behavioral
  change, appended to `todo.md` (section 「代码深审 2026-11-01」).
- **Separate build blocker**: `main.wxs` candle schema violations
  (CUSTOS-MSI-001) fixed at 048b53f — not a review finding; committed in the
  same batch, so 14 fix commits total.
- No code was changed to match documentation; these are all defect fixes.
- Gate at review close: `cargo test --workspace` = 537 passed / 0 failed (47
  test binaries); `cargo fmt --all --check` and `cargo clippy --workspace
  --all-targets -- -D warnings` clean.

## P0

### R1 — `dechunk()` chunk-size overflow panics on hostile HTTP response
`crates/custos-probe/src/http.rs:724` (fixed at f7eb238)

`usize::from_str_radix` accepts a chunk size up to `usize::MAX`; the unchecked
`size_end + 2 + size` overflows (debug: panic; release: wraps past the
`rest.len()` guard), then `&rest[size_end + 2..chunk_end]` slices start > end.
Any `http-endpoint` probe against a peer that declares an oversized chunk
crashes the probe. Fixed with `checked_add`; the oversized or wrapping chunk is
now treated as malformed (returns `None`), never a crash. Regression tests
`huge_chunk_size_is_malformed_not_panic` and `oversized_but_fitting_chunk_size_is_malformed`.

## P1

### R2 — `chunked_complete()` same additive overflow
`crates/custos-probe/src/http.rs:665` (fixed at f7eb238, same root cause as R1)

### R3 — `/proc` reads gated `#[cfg(unix)]` instead of `#[cfg(target_os = "linux")]`
`crates/custos-probe/src/mem.rs:91`, `crates/custos-probe/src/cpu.rs:121,197,218`
(fixed at 30de64f)

On macOS/BSD the memory and CPU probes compiled in `/proc/meminfo`, `/proc/stat`,
`/proc/loadavg` reads and always failed, reporting `Unreachable` (mem.rs:49,
cpu.rs:81). Re-gated to `target_os = "linux"` with the unsupported stub for other
platforms — the same pattern `process.rs:100` and `netif.rs:246` already use
(§11.1: platform support must never be faked).

### R4 — `JsonOp::LessThan` silently passes non-numeric values
`crates/custos-probe/src/http.rs:126` (fixed at 63bb092)

`LessThan` was `!json_greater(actual, value) && !equal`; an incomparable actual
makes both terms false, so a `<` assertion against a string/bool silently
PASSED. Replaced with a symmetric `json_less` (shared `numeric_of`); incomparable
types now fail the assertion. Test `json_assertion_operators_cover_the_matrix`
extended with the non-numeric cases.

### R5 — `max_redirects: u64 as u32` silent truncation
`crates/custos-probe/src/http.rs:226` (fixed at e20f683)

`4294967296 as u32 == 0` (redirects silently off), `4294967297 == 1`. Now
`u32::try_from` with a config error. Test `from_config_rejects_redirect_cap_that_truncates`.

### R6 — exec `try_wait()` error leaks the child and two reader threads
`crates/custos-probe/src/exec.rs:140` (fixed at 82d3ca0)

A wait error returned `StartFailed` while the spawned child kept running and
both reader threads stayed blocked on the pipes (leaked), and a running child
was misreported as start-failed. The error arm now kills the tree, reaps the
child and joins the readers — mirroring the timeout arm.

### R7 — log probe offset set from pre-read file length
`crates/custos-probe/src/log.rs:127` (fixed at 5790e44)

A burst larger than `max_bytes_per_check` had its tail skipped forever (the
offset jumped to the pre-read file end), and growth between stat and read caused
a re-scan. Now `state.offset += new_bytes.len()` — exact consumed bytes.
Test `burst_beyond_per_check_cap_is_not_skipped`.

### R8 — unbounded plugin protocol line buffering
`crates/custos-plugin-api/src/server.rs:35`, `plugins/python/custos_python_plugin.py:129`
(fixed at bab2082)

`stdin.lock().lines()` / `for line in sys.stdin` accumulated a line with no cap;
a misbehaving host grew the plugin's heap without bound (§4.4: 1 MiB per
message). Added `read_capped_line` to `custos-plugin-api::limits` (drains an
oversized line to its newline so framing survives) and used it in both loops.
Also re-used it for the host's stderr reader (session.rs:530, `read_until` with
no cap) — fixed at e981282.

### C1 — corrupt recovery-run payload persisted as `"{}"`
`crates/custos-store/src/store.rs:820` (fixed at 7cfbf76)

`serde_json::to_string(run).unwrap_or_else(|_| "{}".to_owned())` wrote a row
with no id, which `unfinished_runs` later dropped as corrupt — a real recovery
lost silently. Now logs the serialization error and does not persist.

### C3 — telegram 2xx with unreadable body dead-lettered a delivered message
`crates/custos-notify/src/telegram.rs:93` (fixed at ecaf377)

The `.read_to_string().ok()` swallow made `code = None`, classified `Rejected`,
and a 2xx (which Telegram returns only on acceptance) was dead-lettered. A 2xx
is now the delivery confirmation; the body is read only for non-2xx error detail.

### C10 — `mark_notification_delivered` had no state lease
`crates/custos-store/src/store.rs:476` (fixed at 2a72aeb)

The UPDATE was `WHERE id = ?` with no `AND state = 'sending'`, so a late call
from a superseded claim overwrote an already-delivered row. Now CAS on `sending`;
a delivered row can no longer be flipped back.

### C11 — `redrive()` orphaned a run when the resource was already active
`crates/custos-engine/src/engine.rs:224` (fixed at 716740b)

A woken `Waiting` run popped from `resume_queue` returned early when
`active_runs` contained the id and was never re-queued — orphaned forever. Now
re-pushed to `resume_queue`.

### C12 — `mark_notification_retry` wrote a future timestamp into `updated_at_unix_ms`
`crates/custos-store/src/store.rs:500` (fixed at 2a72aeb)

`next_attempt_at_unix_ms` was bound to both `next_attempt_at_unix_ms` and
`updated_at_unix_ms`, corrupting last-modified semantics for any stale check.
`Queue::mark_retry` now takes `now_unix_ms` (same as `mark_dead`) and binds it
to `updated_at_unix_ms`; the trait, both impls, the engine caller and the test
mocks updated. Regression test `delivered_notification_is_not_flipped_back_by_stale_retry`.

### C25 — `probe_one` panicked on a managed resource with no probe
`crates/custos-engine/src/engine.rs:362` (fixed at 33abd54)

`self.probes.get(id).expect("probe wired for every resource at construction")`
took down the whole recovery loop on a config referencing an unprobed resource
(no startup validation exists). Now reports an honest `Unreachable` report.

### C26 — alternative-group member index panic
`crates/custos-engine/src/engine.rs:1072,1078,1092` (fixed at 33abd54)

`self.managed[m]` indexed a free-form `Vec<ResourceId>` with no membership check;
an unmanaged member panicked during dependency evaluation. Now `.get(m)` — a
missing member counts as unhealthy and offers no recovery.

### C28 — `resume_unfinished` marked non-executing runs `OutcomeUnknown`
`crates/custos-engine/src/engine.rs:1222` (fixed at 716740b)

A run that crashed while `Planning`/`Diagnosing`/`Verifying` (no action ever
executed) was written to terminal `OutcomeUnknown` — bypassing the model's
`allowed_transitions` and never re-driven. Now only `Executing` runs with an
unknown attempt outcome are marked `OutcomeUnknown`; the rest are re-queued.

## P2 — recorded in todo.md

| ID | Site | Issue |
|----|------|-------|
| R9  | netif.rs:143 | `?` aborts the whole route parse on the first row lacking a double-space separator; a localized/malformed row reports NotFound instead of skipping |
| R10 | docker.rs:121 | `restarts.parse::<u64>().unwrap_or(0)` masks crash loops |
| R11 | plugins/cloudflared/src/main.rs:54,59-69 | malformed `cloudflared_tunnel_ha_connections` leaves `connections = None` → match reports HEALTHY — the fault the probe detects becomes healthy on parse failure |
| R12 | windows.rs:523-524 | `parse_netsh_interfaces` recognizes only English/zh-CN tokens; ja/fr/de/... report every interface down |
| R13 | windows.rs:435-444 | `netsh_interface_reset_argv` puts both `admin=disabled` and `admin=enabled` in one `netsh interface set interface` call; a single invocation takes one `admin=` value |
| R14 | openwrt.rs:68-73 | non-JSON `ubus call service list` mapped to "Stopped" via `unwrap_or(false)` instead of Unknown/error |
| R15 | plugins/mihomo/src/main.rs:129-135 | `plain.first()` picks the first non-current member, not the next after current (wrap); repeated recovery never advances rotation |
| R16 | linux.rs:331, windows.rs:542 | `probe_tcp` builds `format!("{}:{}", host, port).parse()`; a bare IPv6 literal is never a valid `SocketAddr` string → IPv6 targets unreachable-by-construction |
| S2  | herald.rs:44 | `endpoint()` interpolates `config.base_url` into the POST URL with no SSRF guard (webhook has one via webhook_ssrf.rs) |
| C2  | store.rs:257,1060 | a corrupt `outcome` string parses to `None`, misclassified as "crashed mid-execution"; the original outcome text is discarded |
| C4  | herald.rs:94-99 | 2xx with unreadable body → `code=None` → `Http`/retryable → spurious retries (safe direction; kept as working-as-intended, low priority) |
| C5  | engine.rs:289,1089 | `let _ = self.try_recover(...)` discards dependency/alternative-group recovery errors; the waiting run stays Waiting with no error surfaced |
| C7  | store.rs:437-446 + webhook.rs:134-158 | stale-`sending` reclaim (60s) races a legitimately long delivery (retries=6, backoff=30000 doubles past 60s) → concurrent duplicate delivery; needs a lease/reclaim design |
| C8  | supervisor.rs:96,185 | plugin-state mutex held across process spawn + two RPC round-trips (~20s of blocking I/O), stalling the engine tick and `status()` |
| C13 | budget.rs:54,134-138 | attempt history grows unbounded, never pruned or reset (compounds C29) |
| C15 | engine.rs:263-311 | waiter dependencies re-probed every tick bypassing `ProbeDueTracker`, persisting a `probe_results` row per call |
| C16 | notify engine.rs:255-258 | transient failures always `mark_retry` with no attempt cap → rows and counters grow unbounded (ties to C17) |
| C17 | notify engine.rs:244-259 | transient failures retry forever — no max-attempts dead-letter; a permanently-down endpoint is never escalated to a human |
| C18 | notify engine.rs:137-221 | synchronous fsync'd SQLite inserts run inline on the engine tick thread; a burst stalls the single tick |
| C19 | notify engine.rs:225-281 | one channel's retry sleep chain blocks the entire serial delivery queue (webhook in-channel `thread::sleep` doubling) |
| C20 | engine.rs:571-574,745-747 | recovery actions run on the tick thread with a plan `timeout_ms` and no watchdog; a hung action stalls the tick indefinitely |
| C21 | executor.rs:41 | `StartFailed → Failed` is retryable, so a permanently missing binary is re-attempted across the whole plan |
| C22 | webhook.rs:134-158 | double backoff accounting: in-channel retry re-sends on the same error the queue also schedules for retry |
| C23 | dashboard/src/lib.rs:92-105 | per-read 2s timeout but no total request deadline; a client trickling < 64 KiB stalls the single-threaded accept loop |
| C24 | session.rs:245-309 | write-ack wait uses `default_timeout()` regardless of the caller's `timeout`, silently cutting a longer request short |
| C27 | supervisor.rs:128-144 | `HostError::Unavailable` treated as a crash, but a live plugin also returns it via a declared RPC error → burns the crash budget |
| C29 | budget.rs | attempt budget documented "per incident" but never reset on successful recovery; a flapping-then-stable resource can be wrongly declared exhausted |
| C30 | dashboard/src/lib.rs:70-83 | `for stream in self.listener.incoming() { Err(_) => continue }` with no backoff; persistent accept error (EMFILE) spins a tight loop |

## P3 — recorded in todo.md

| ID | Site | Issue |
|----|------|-------|
| R17 | platform/error.rs:63-64 | `unreachable!()` inside `From<ExecResult> for PlatformError` for the `Succeeded` variant |
| R18 | http.rs:120,127 | `JsonOp::Exists => unreachable!()` arms (structurally unreachable today, one refactor from reachable) |
| S1  | plugins/python/examples/file_health_probe.py:20,31 | `target` taken from params and used unsandboxed in `os.path.getmtime` / `open(..., "a")`; no canonicalization, `..` or absolute-path rejection |
| C6  | notify engine.rs:274-277 | a failed queue-write after a delivery is only logged; no reconciliation (row left `sending` until stale reclaim) |
| C9  | platform_tools.rs | `std::sync::MutexGuard` held across slow platform I/O; `try_lock` surfaces contention as a silently skipped operation rather than a wait |

## Fix commits

```
f7eb238 fix: checked chunk size math in http probe            (R1, R2)
30de64f fix: gate proc reads to linux in cpu and mem probes   (R3)
63bb092 fix: json less-than fails on non-numeric values       (R4)
e20f683 fix: reject redirect cap that truncates in http config (R5)
82d3ca0 fix: kill child on exec wait error instead of leaking (R6)
5790e44 fix: advance log offset by bytes actually read        (R7)
bab2082 fix: bound plugin protocol line reads to 1 MiB        (R8)
e981282 fix: bound stderr line growth in plugin session       (R8b)
33abd54 fix: engine reports unreachable instead of panicking on unwired probe and unmanaged group member (C25, C26)
716740b fix: requeue redrive drops and resume non-executing unfinished runs (C11, C28)
2a72aeb fix: lease guard on notification delivery and retry state (C10, C12)
ecaf377 fix: telegram 2xx delivery is not a dead letter       (C3)
7cfbf76 fix: never persist a corrupt recovery run payload     (C1)
048b53f fix: wxs schema rules for majorupgrade and shortcuts  (CUSTOS-MSI-001 build blocker)
```

Counts: **P0 1/1 fixed. P1 15/15 fixed. P2 28 recorded. P3 5 recorded.
Total 49 found, 16 fixed, 33 recorded.**
