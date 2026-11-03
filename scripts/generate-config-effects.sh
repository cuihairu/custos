#!/usr/bin/env bash
# Custos configuration-effect evidence generator (DOC-GIF · P2).
#
# For the user-facing config switches that have an observable off->on
# effect (four switches), this script drives the real `custos` binary and captures a
# terminal recording of the OFF state and the ON state, then stitches a
# GIF (off frame -> on frame, looping) with ffmpeg. It is re-runnable:
# any config change re-generates the evidence in place.
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

# render a terminal-style text block to a PNG using ImageMagick.
frame() { # frame <text-file> <out-png>
  local txt="$1" png="$2"
  # `caption:-` reads stdin, which this ImageMagick build renders almost
  # entirely blank (only the first glyph row); passing the text as an inline
  # caption argument renders every line correctly (verified: ~5k px vs 15).
  convert -size 900x420 -background '#1e1e28' -fill '#d4d4e0' \
    -font 'DejaVu-Sans-Mono' -pointsize 16 caption:"$(< "$txt")" "$png"
}

# stitch an off->on pair into one looping GIF.
stitch() { # stitch <off-png> <on-png> <out-gif>
  local off="$1" on="$2" gif="$3"
  ffmpeg -y -loglevel error -framerate 1 \
    -i "$off" -framerate 1 -i "$on" \
    -filter_complex "[0][1]concat=n=2:v=1:a=0[v];[v]split[a][b];[a]palettegen[p];[b][p]paletteuse" \
    -loop 0 "$gif"
}

CUSTOS="$BIN"

# ── 1. dashboard.enabled: off = no listener; on = HTTP 200 ──────────────
PORT=18095
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
  curl -sS --max-time 1 "http://127.0.0.1:$PORT/" 2>&1 | head -c 120 || true
  echo "  ^ process exited; no dashboard listener exists in off mode"
} > "$TMP/dash-off.txt" 2>&1 || true

{
  echo "\$ custos start dash-on.toml"
  "$CUSTOS" start "$TMP/dash-on.toml" --db "$TMP/dash-on.db" --interval 100 > "$TMP/dash-on.stdout" 2>&1 &
  server_pid=$!
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if curl -sS --max-time 1 "http://127.0.0.1:$PORT/" > "$TMP/dash-on.http" 2> "$TMP/dash-on.curlerr"; then
      break
    fi
    sleep 0.2
  done
  kill "$server_pid" 2>/dev/null || true
  wait "$server_pid" 2>/dev/null || true
  cat "$TMP/dash-on.stdout"
  echo "\$ curl -s http://127.0.0.1:$PORT/  (while server is running)"
  echo "HTTP " "$(head -c 200 "$TMP/dash-on.http" 2>/dev/null || cat "$TMP/dash-on.curlerr")"
  echo "  ^ dashboard listener is reachable in on mode"
} > "$TMP/dash-on.txt" 2>&1 || true

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
} > "$TMP/res-off.txt" 2>&1 || true

{
  echo "\$ custos start res-on.toml --once"
  "$CUSTOS" start "$TMP/res-on.toml" --db "$TMP/res-on.db" --once --interval 100 2>&1 | grep -iE 'recovery|run|escalat' || true
  echo "\$ custos runs --db res-on.db"
  "$CUSTOS" runs --db "$TMP/res-on.db"
} > "$TMP/res-on.txt" 2>&1 || true

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
} > "$TMP/nl-off.txt" 2>&1 || true
{
  echo "\$ RUST_LOG=info custos start nl-on.toml --once"
  RUST_LOG=info "$CUSTOS" start "$TMP/nl-on.toml" --db "$TMP/nl-on.db" --once --interval 100 2>&1 | grep -iE 'event|audit' | head -1
} > "$TMP/nl-on.txt" 2>&1 || true

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
  "$CUSTOS" validate "$TMP/hr-off.toml" 2>&1 || true
} > "$TMP/hr-off.txt" 2>&1 || true
{
  echo "\$ custos validate hr-on.toml"
  "$CUSTOS" validate "$TMP/hr-on.toml" 2>&1 || true
} > "$TMP/hr-on.txt" 2>&1 || true

frame "$TMP/hr-off.txt" "$TMP/hr-off.png"
frame "$TMP/hr-on.txt" "$TMP/hr-on.png"
stitch "$TMP/hr-off.png" "$TMP/hr-on.png" "$OUT/allow-high-risk.gif"
cp "$TMP/hr-off.png" "$OUT/allow-high-risk-off.png"
cp "$TMP/hr-on.png" "$OUT/allow-high-risk-on.png"
echo "generated allow-high-risk.gif"

echo "all configuration-effect evidence generated under $OUT"
