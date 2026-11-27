#!/bin/sh
# Copyright 2026 cuihairu <chuihairu@gmail.com>
# SPDX-License-Identifier: MIT

# Regression gate for the engine ipk postinst: a prerm-stopped engine on an
# enabled service must be restarted after upgrade (f941d3b). Runs a rewritten
# copy of CONTROL/postinst under a temporary fake root so nothing touches the
# host's /etc or /usr. Only the absolute prefixes change; the logic under
# test stays byte-identical.
# Run:  sh packaging/openwrt/tests/postinst-test.sh

set -eu

ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT
SRC=$(cd "$(dirname "$0")/.." && pwd)/CONTROL/postinst
SCRIPT="$ROOT/postinst"

# Redirect the four mkdir targets and the service path into the fake root.
# A future absolute prefix added to postinst must be added here too.
sed -e 's|/etc/custos|'"$ROOT"'/etc/custos|g' \
    -e 's|/usr/lib/lua/luci|'"$ROOT"'/usr/lib/lua/luci|g' \
    -e 's|/etc/init.d/custos|'"$ROOT"'/etc/init.d/custos|g' \
    "$SRC" > "$SCRIPT"
chmod 755 "$SCRIPT"
mkdir -p "$ROOT/etc/init.d"

passed=0
ok() {
	if [ "$1" != 0 ]; then
		echo "FAIL: $2" >&2
		exit 1
	fi
	passed=$((passed + 1))
	echo "ok $passed - $2"
}

# Stub init script: logs each subcommand, emulates enabled/disabled and
# running/inactive status from the scenario files.
cat > "$ROOT/etc/init.d/custos" <<STUB
#!/bin/sh
echo "\$1" >> "$ROOT/calls"
case "\$1" in
	enabled) [ -f "$ROOT/enabled" ] ;;
	status)  [ -f "$ROOT/running" ] && echo running || echo inactive ;;
esac
STUB
chmod 755 "$ROOT/etc/init.d/custos"

run_scenario() { : > "$ROOT/calls"; sh "$SCRIPT" >/dev/null 2>&1; }
starts() { grep -c '^start$' "$ROOT/calls" 2>/dev/null || true; }

# Enabled but not running (the post-upgrade state): exactly one start.
: > "$ROOT/enabled"
run_scenario
ok "$([ "$(starts)" = 1 ] && echo 0 || echo 1)" "enabled+inactive restarts the engine"
ok "$(grep -q '^enable$' "$ROOT/calls" && echo 0 || echo 1)" "enabled+inactive still enables the service"

# Already running: no extra start (idempotent on a live service).
: > "$ROOT/running"
run_scenario
ok "$([ "$(starts)" = 0 ] && echo 0 || echo 1)" "enabled+running does not restart"

# Disabled: postinst enables it, but must not force a start.
rm -f "$ROOT/enabled" "$ROOT/running"
run_scenario
ok "$([ "$(starts)" = 0 ] && echo 0 || echo 1)" "disabled service stays stopped"

# Missing init script: postinst must still exit 0 (fresh install before
# the init script lands, or a package laid out differently).
rm -f "$ROOT/etc/init.d/custos"
ok "$(sh "$SCRIPT" >/dev/null 2>&1 && echo 0 || echo 1)" "missing init script is not an error"

echo "$passed tests passed"
