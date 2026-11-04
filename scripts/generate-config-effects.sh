#!/usr/bin/env bash
# Custos configuration-effect evidence generator (DOC-GIF · P2).
#
# For the user-facing config switches that have an observable off->on
# effect (four switches), this script drives the real `custos` binary and captures a
# terminal recording of the OFF state and the ON state, then stitches a
# GIF (off frame -> on frame, looping) with ffmpeg. It is re-runnable:
# any config change re-generates the evidence in place.
#
# The script also doubles as a configuration-effect regression test: each
# switch's OFF/ON run is followed by an assertion that the documented
# observable effect actually happened (no recovery run when the resource is
# disabled / a recovery run recorded when enabled; no audit line when the
# local-log channel is disabled / one when enabled; high-risk plan rejected
# by `validate` when disallowed / accepted when allowed; dashboard listener
# absent when disabled / reachable when enabled). Any failure exits non-zero.
#
# Requirements: cargo-built `custos` binary (target/debug/custos), ffmpeg,
# ImageMagick (convert) for frame scaling. Evidence lands in docs/public/effects/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${CUSTOS_BIN:-$ROOT/target/debug/custos}"
OUT="$ROOT/docs/public/effects"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$OUT"
command -v ffmpeg >/dev/null || { echo "ffmpeg required"; exit 1; }
command -v convert >/dev/null || { echo "ImageMagick (convert) required"; exit 1; }
[ -x "$BIN" ] || { echo "build custos first: cargo build -p custos-cli"; exit 1; }

# Escapes a string for use inside an ImageMagick `caption:` argument: `%` is
# the inline format-escape introducer, so a literal percent must be doubled.
# This keeps captured terminal text (URLs with %20, percentages, etc.) from
# being silently re-rendered as a different caption.
im_escape_percent() {
  printf '%s' "$1" | sed 's/%/%%/g'
}

# render a terminal-style text block to a PNG using ImageMagick.
frame() { # frame <text-file> <out-png>
  local txt="$1" png="$2"
  # `caption:-` reads stdin, which this ImageMagick build renders almost
  # entirely blank (only the first glyph row); passing the text as an inline
  # caption argument renders every line correctly (verified: ~5k px vs 15).
  # `%` must be escaped so ImageMagick does not expand it as a format string.
  local caption
  caption="$(im_escape_percent "$(< "$txt")")"
  convert -size 900x420 -background '#1e1e28' -fill '#d4d4e0' \
    -font 'DejaVu-Sans-Mono' -pointsize 16 caption:"$caption" "$png"
}

# stitch an off->on pair into one looping GIF.
stitch() { # stitch <off-png> <on-png> <out-gif>
  local off="$1" on="$2" gif="$3"
  ffmpeg -y -loglevel error -framerate 1 \
    -i "$off" -framerate 1 -i "$on" \
    -filter_complex "[0][1]concat=n=2:v=1:a=0[v];[v]split[a][b];[a]palettegen[p];[b][p]paletteuse" \
    -loop 0 "$gif"
}

# Asserts that a grep pattern appears in a file, with a descriptive message.
assert_grep() { # assert_grep <needle> <file> <what>
  local needle="$1" file="$2" what="$3"
  if ! grep -q "$needle" "$file"; then
    echo "ASSERTION FAILED: $what" >&2
    echo "  expected to find: $needle" >&2
    echo "  in: $file" >&2
    echo "--- file ---" >&2
    sed 's/^/  /' "$file" >&2
    exit 1
  fi
}

# Asserts that a grep pattern does NOT appear in a file.
assert_not_grep() { # assert_not_grep <needle> <file> <what>
  local needle="$1" file="$2" what="$3"
  if grep -q "$needle" "$file"; then
    echo "ASSERTION FAILED: $what" >&2
    echo "  expected NOT to find: $needle" >&2
    echo "  in: $file" >&2
    echo "--- file ---" >&2
    sed 's/^/  /' "$file" >&2
    exit 1
  fi
}

# Picks a dashboard port that is not already listening, so a stale listener
# (leftover `custos start` from an interrupted run, or an unrelated process)
# cannot corrupt the dashboard OFF frame by returning HTTP 200 and defeating
# the off/on contrast. Falls back to the hard-coded default for the docs.
pick_dashboard_port() { # pick_dashboard_port <default> -> echoes a free port
  local default="$1" candidate rc
  for candidate in "$default" 18096 18097 18098 18099 18100; do
    set +e
    curl -sS --max-time 1 "http://127.0.0.1:$candidate/" >/dev/null 2>&1
    rc=$?
    set -e
    # curl exit 7 = connection refused -> port is free.
    if [ "$rc" -ne 0 ]; then
      echo "$candidate"
      return 0
    fi
  done
  echo "no free dashboard port found (tried $default 18096-18100)" >&2
  echo "  stop the process occupying one, then re-run" >&2
  exit 1
}

CUSTOS="$BIN"

# ── 1. dashboard.enabled: off = no listener; on = HTTP 200 ──────────────
PORT="$(pick_dashboard_port 18095)"
cat > "$TMP/dash-off.toml" <<EOF
version = 1
[[resources]]
id = "demo"
type = "custom-command"
enabled = false
probe.command = ["true"]
[dashboard]
enabled = false
listen = "127.0.0.1:$PORT"
EOF
cat > "$TMP/dash-on.toml" <<EOF
version = 1
[[resources]]
id = "demo"
type = "custom-command"
enabled = false
probe.command = ["true"]
[dashboard]
enabled = true
listen = "127.0.0.1:$PORT"
EOF

{
  echo "\$ custos start dash-off.toml --once"
  "$CUSTOS" start "$TMP/dash-off.toml" --db "$TMP/dash-off.db" --once --interval 100
  echo "\$ curl -s http://127.0.0.1:$PORT/  (after --once exits)"
  set +e
  curl -sS --max-time 1 "http://127.0.0.1:$PORT/" > "$TMP/dash-off.http" 2>&1
  dash_off_rc=$?
  set -e
  echo "  (curl exit=$dash_off_rc)"
  head -c 120 "$TMP/dash-off.http" || true
  echo "  ^ process exited; no dashboard listener exists in off mode"
} > "$TMP/dash-off.txt" 2>&1
# Regression assertion: off state must NOT be reachable (curl fails, exit!=0).
assert_grep "curl exit=" "$TMP/dash-off.txt" \
  "dashboard OFF frame must show curl exit"
if grep -q "curl exit=0" "$TMP/dash-off.txt"; then
  echo "ASSERTION FAILED: dashboard OFF frame must show no listener (curl succeeded)" >&2
  echo "  expected curl exit != 0 in: $TMP/dash-off.txt" >&2
  sed 's/^/  /' "$TMP/dash-off.txt" >&2
  exit 1
fi

{
  echo "\$ custos start dash-on.toml"
  "$CUSTOS" start "$TMP/dash-on.toml" --db "$TMP/dash-on.db" --interval 100 > "$TMP/dash-on.stdout" 2>&1 &
  server_pid=$!
  dash_on_http=000
  for _ in $(seq 1 40); do
    dash_on_http="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 1 "http://127.0.0.1:$PORT/" 2>/dev/null)" || true
    if [ "$dash_on_http" = "200" ]; then
      break
    fi
    sleep 0.25
  done
  kill "$server_pid" 2>/dev/null || true
  wait "$server_pid" 2>/dev/null || true
  cat "$TMP/dash-on.stdout"
  echo "\$ curl -s http://127.0.0.1:$PORT/  (while server is running)"
  echo "HTTP $dash_on_http"
  echo "  ^ dashboard listener is reachable in on mode"
} > "$TMP/dash-on.txt" 2>&1
# Regression assertion: on state must be reachable (HTTP 200).
assert_grep "HTTP 200" "$TMP/dash-on.txt" \
  "dashboard ON frame must show HTTP 200 (listener reachable)"

frame "$TMP/dash-off.txt" "$TMP/dash-off.png"
frame "$TMP/dash-on.txt" "$TMP/dash-on.png"
stitch "$TMP/dash-off.png" "$TMP/dash-on.png" "$OUT/dashboard-enabled.gif"
cp "$TMP/dash-off.png" "$OUT/dashboard-enabled-off.png"
cp "$TMP/dash-on.png" "$OUT/dashboard-enabled-on.png"
echo "generated dashboard-enabled.gif"

# ── 2. resources[].enabled: off = no recovery run; on = run recorded ────
cat > "$TMP/res-off.toml" <<EOF
version = 1
[[resources]]
id = "demo"
type = "custom-command"
enabled = false
probe.command = ["false"]
[resources.health_policy]
failure_threshold = 1
success_threshold = 1
[[resources.recovery_policy.plans]]
plan_id = "restart"
applicable_faults = ["unknown_failure"]
action = { program = "/bin/true" }
EOF
cat > "$TMP/res-on.toml" <<EOF
version = 1
[[resources]]
id = "demo"
type = "custom-command"
enabled = true
probe.command = ["false"]
[resources.health_policy]
failure_threshold = 1
success_threshold = 1
[[resources.recovery_policy.plans]]
plan_id = "restart"
applicable_faults = ["unknown_failure"]
action = { program = "/bin/true" }
EOF

{
  echo "\$ custos start res-off.toml --once"
  "$CUSTOS" start "$TMP/res-off.toml" --db "$TMP/res-off.db" --once --interval 100
  echo "\$ custos runs --db res-off.db"
  "$CUSTOS" runs --db "$TMP/res-off.db"
} > "$TMP/res-off.txt" 2>&1
# Regression assertion: disabled resource must NOT record a recovery run.
assert_grep "no recovery runs recorded" "$TMP/res-off.txt" \
  "resource OFF frame must show no recovery run"

{
  echo "\$ custos start res-on.toml --once"
  "$CUSTOS" start "$TMP/res-on.toml" --db "$TMP/res-on.db" --once --interval 100 2>&1 | grep -iE 'recovery|run|escalat' || true
  echo "\$ custos runs --db res-on.db"
  "$CUSTOS" runs --db "$TMP/res-on.db"
} > "$TMP/res-on.txt" 2>&1
# Regression assertion: enabled resource must record a recovery run.
assert_grep "run-.*demo" "$TMP/res-on.txt" \
  "resource ON frame must show a recovery run for demo"

frame "$TMP/res-off.txt" "$TMP/res-off.png"
frame "$TMP/res-on.txt" "$TMP/res-on.png"
stitch "$TMP/res-off.png" "$TMP/res-on.png" "$OUT/resource-enabled.gif"
cp "$TMP/res-off.png" "$OUT/resource-enabled-off.png"
cp "$TMP/res-on.png" "$OUT/resource-enabled-on.png"
echo "generated resource-enabled.gif"

# ── 3. notification channel local-log enabled ──────────────────────────
cat > "$TMP/nl-off.toml" <<EOF
version = 1
[[resources]]
id = "demo"
type = "custom-command"
probe.command = ["false"]
[resources.health_policy]
failure_threshold = 1
success_threshold = 1
[notification.channels.audit]
type = "local-log"
enabled = false
EOF
cat > "$TMP/nl-on.toml" <<EOF
version = 1
[[resources]]
id = "demo"
type = "custom-command"
probe.command = ["false"]
[resources.health_policy]
failure_threshold = 1
success_threshold = 1
[notification.channels.audit]
type = "local-log"
enabled = true
EOF

{
  echo "\$ RUST_LOG=info custos start nl-off.toml --once"
  RUST_LOG=info "$CUSTOS" start "$TMP/nl-off.toml" --db "$TMP/nl-off.db" --once --interval 100 2>&1 | grep -iE 'event|audit' || echo "  (no structured audit line: local-log disabled)"
} > "$TMP/nl-off.txt" 2>&1
# Regression assertion: disabled local-log must emit no audit line.
assert_grep "no structured audit line" "$TMP/nl-off.txt" \
  "notification OFF frame must show no audit line"
{
  echo "\$ RUST_LOG=info custos start nl-on.toml --once"
  RUST_LOG=info "$CUSTOS" start "$TMP/nl-on.toml" --db "$TMP/nl-on.db" --once --interval 100 2>&1 | grep -iE 'event|audit' | head -1
} > "$TMP/nl-on.txt" 2>&1
# Regression assertion: enabled local-log must emit a structured audit line.
assert_grep "event_type" "$TMP/nl-on.txt" \
  "notification ON frame must show a structured audit line"

frame "$TMP/nl-off.txt" "$TMP/nl-off.png"
frame "$TMP/nl-on.txt" "$TMP/nl-on.png"
stitch "$TMP/nl-off.png" "$TMP/nl-on.png" "$OUT/local-log-enabled.gif"
cp "$TMP/nl-off.png" "$OUT/local-log-enabled-off.png"
cp "$TMP/nl-on.png" "$OUT/local-log-enabled-on.png"
echo "generated local-log-enabled.gif"

# ── 4. recovery_policy.allow_high_risk: off = rejected; on = accepted ──
cat > "$TMP/hr-off.toml" <<EOF
version = 1
[[resources]]
id = "demo"
type = "custom-command"
probe.command = ["false"]
[resources.recovery_policy]
allow_high_risk = false
[[resources.recovery_policy.plans]]
plan_id = "restart"
applicable_faults = ["unknown_failure"]
risk = "high"
action = { program = "/bin/true" }
EOF
cat > "$TMP/hr-on.toml" <<EOF
version = 1
[[resources]]
id = "demo"
type = "custom-command"
probe.command = ["false"]
[resources.recovery_policy]
allow_high_risk = true
[[resources.recovery_policy.plans]]
plan_id = "restart"
applicable_faults = ["unknown_failure"]
risk = "high"
action = { program = "/bin/true" }
EOF

{
  echo "\$ custos validate hr-off.toml"
  set +e
  "$CUSTOS" validate "$TMP/hr-off.toml" 2>&1
  local_rc=$?
  set -e
  echo "  (validate exit=$local_rc)"
} > "$TMP/hr-off.txt" 2>&1
# Regression assertion: disallowed high-risk plan must be rejected (exit != 0).
assert_grep "validate exit=1" "$TMP/hr-off.txt" \
  "high-risk OFF frame must show validate rejection"
{
  echo "\$ custos validate hr-on.toml"
  set +e
  "$CUSTOS" validate "$TMP/hr-on.toml" 2>&1
  local_rc=$?
  set -e
  echo "  (validate exit=$local_rc)"
} > "$TMP/hr-on.txt" 2>&1
# Regression assertion: allowed high-risk plan must be accepted (exit 0).
assert_grep "validate exit=0" "$TMP/hr-on.txt" \
  "high-risk ON frame must show validate acceptance"

frame "$TMP/hr-off.txt" "$TMP/hr-off.png"
frame "$TMP/hr-on.txt" "$TMP/hr-on.png"
stitch "$TMP/hr-off.png" "$TMP/hr-on.png" "$OUT/allow-high-risk.gif"
cp "$TMP/hr-off.png" "$OUT/allow-high-risk-off.png"
cp "$TMP/hr-on.png" "$OUT/allow-high-risk-on.png"
echo "generated allow-high-risk.gif"

echo "all configuration-effect evidence generated under $OUT"
echo "regression assertions: 4/4 passed"
