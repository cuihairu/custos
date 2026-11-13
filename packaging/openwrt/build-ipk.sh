#!/bin/sh
# Builds the three OpenWrt .ipk packages for custos:
#   custos            musl static binary + procd init + UCI config + default TOML
#   luci-app-custos   LuCI controller, settings page and status dashboard (arch all)
#   app-meta-custos   iStore app metadata (icon + app json), arch all
#
# Usage: build-ipk.sh <path-to-musl-custos-binary> [output-dir] [--arch x86_64|aarch64] [--rel N]
#
# The package architecture is derived from the binary itself (`file` reports
# the ELF machine); --arch overrides it for cross-built artifacts where
# `file` may be unavailable.
set -eu

die() { printf 'build-ipk: %s\n' "$*" >&2; exit 1; }

BINARY=""
OUT="."
ARCH_OVERRIDE=""
REL_OVERRIDE=""
while [ $# -gt 0 ]; do
	case "$1" in
		--rel)
			[ $# -ge 2 ] || die "--rel requires a value"
			REL_OVERRIDE="$2"
			shift 2
			;;
		--arch)
			[ $# -ge 2 ] || die "--arch requires a value (x86_64|aarch64)"
			ARCH_OVERRIDE="$2"
			shift 2
			;;
		--*)
			die "unknown option: $1"
			;;
		*)
			if [ -z "$BINARY" ]; then
				BINARY="$1"
			else
				OUT="$1"
			fi
			shift
			;;
	esac
done

[ -n "$BINARY" ] || die "usage: build-ipk.sh <binary> [output-dir] [--arch x86_64|aarch64]"
[ -f "$BINARY" ] || die "binary not found: $BINARY"

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$VERSION" ] || die "cannot read version from Cargo.toml"
REL="${VERSION}-1"
[ -n "$REL_OVERRIDE" ] && REL="${VERSION}-${REL_OVERRIDE}"

if [ -n "$ARCH_OVERRIDE" ]; then
	case "$ARCH_OVERRIDE" in
		x86_64 | aarch64) ARCH="$ARCH_OVERRIDE" ;;
		*) die "unsupported --arch: $ARCH_OVERRIDE (expected x86_64|aarch64)" ;;
	esac
else
	# `file` reports the ELF machine, mapped to opkg's architecture names.
	ARCH=$(file -b "$BINARY" | sed -n 's/.*[, ]\(x86-64\|aarch64\).*/\1/p' | head -1)
	case "$ARCH" in
		x86-64) ARCH=x86_64 ;;
		aarch64) ARCH=aarch64 ;;
		*) die "unsupported binary architecture: $ARCH" ;;
	esac
fi

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

SRC=packaging/openwrt

# ---- custos (arch-specific) ------------------------------------------------
R="$STAGE/custos"
mkdir -p "$R/data/etc/init.d" "$R/data/etc/config" "$R/data/etc/custos" \
	"$R/data/usr/bin" "$R/CONTROL"
cp "$SRC/etc/init.d/custos" "$R/data/etc/init.d/"
cp "$SRC/etc/config/custos" "$R/data/etc/config/"
cp "$SRC/etc/custos/custos.toml" "$R/data/etc/custos/"
cp "$BINARY" "$R/data/usr/bin/custos"
chmod 755 "$R/data/etc/init.d/custos" "$R/data/usr/bin/custos"

cat > "$R/CONTROL/conffiles" <<'EOF'
/etc/config/custos
/etc/custos/custos.toml
/etc/init.d/custos
EOF
cp "$SRC/CONTROL/postinst" "$R/CONTROL/postinst"
cp "$SRC/CONTROL/prerm" "$R/CONTROL/prerm"
SIZE=$(du -sk "$R/data" | cut -f1)
cat > "$R/CONTROL/control" <<EOF
Package: custos
Version: $REL
Depends: libc
Source: https://github.com/cuihairu/custos
SourceName: custos
License: MIT
Section: admin
SourceDateEpoch: 1794931200
Maintainer: cuihairu <chuihairu@gmail.com>
Architecture: $ARCH
Installed-Size: $SIZE
Description: Cross-platform local self-healing engine: dependency-aware failure detection and automatic recovery, with a procd service.
 跨平台本地自愈引擎：依赖感知的故障检测与自动恢复，含 procd 服务。
EOF

chmod 755 "$R"/CONTROL/postinst "$R"/CONTROL/prerm
mkdir -p "$OUT"
tar -czf "$R/control.tar.gz" -C "$R/CONTROL" ./control ./postinst ./prerm ./conffiles
# Tar the whole staged tree (directory entries included): opkg does not
# create missing parent dirs for file-only listings (new subdirectories).
# Root ownership regardless of the builder's uid (files ship to the router).
tar --owner=0 --group=0 --numeric-owner -czf "$R/data.tar.gz" -C "$R/data" etc usr
echo "2.0" > "$R/debian-binary"
tar -czf "$OUT/custos_${REL}_${ARCH}.ipk" -C "$R" \
	./debian-binary ./control.tar.gz ./data.tar.gz
echo "built: $OUT/custos_${REL}_${ARCH}.ipk"

# ---- luci-app-custos (all) -------------------------------------------------
R="$STAGE/luci"
mkdir -p "$R/data/usr/lib/lua/luci/controller" "$R/data/usr/lib/lua/luci/model/cbi" \
	"$R/data/usr/lib/lua/luci/view/custos" "$R/data/usr/lib/lua/luci/custos" "$R/CONTROL"
cp "$SRC/usr/lib/lua/luci/controller/custos.lua" "$R/data/usr/lib/lua/luci/controller/"
cp "$SRC/usr/lib/lua/luci/model/cbi/custos.lua" "$R/data/usr/lib/lua/luci/model/cbi/"
cp "$SRC/usr/lib/lua/luci/view/custos/status.htm" "$R/data/usr/lib/lua/luci/view/custos/"
cp "$SRC/usr/lib/lua/luci/view/custos/resources.htm" "$R/data/usr/lib/lua/luci/view/custos/"
cp "$SRC/usr/lib/lua/luci/custos/tomledit.lua" "$R/data/usr/lib/lua/luci/custos/"

cat > "$R/CONTROL/postinst" <<'EOF'
#!/bin/sh
# Drop the LuCI menu/index caches so the new controller routes are picked
# up without a reboot (idempotent; nothing to do when the cache is absent).
rm -f /tmp/luci-indexcache* 2>/dev/null || true
exit 0
EOF
cat > "$R/CONTROL/prerm" <<'EOF'
#!/bin/sh
exit 0
EOF
SIZE=$(du -sk "$R/data" | cut -f1)
cat > "$R/CONTROL/control" <<EOF
Package: luci-app-custos
Version: $REL
Depends: custos
Source: https://github.com/cuihairu/custos
SourceName: luci-app-custos
License: MIT
Section: admin
SourceDateEpoch: 1794931200
Maintainer: cuihairu <chuihairu@gmail.com>
Architecture: all
Installed-Size: $SIZE
Description: LuCI interface for custos: settings page, live status dashboard (health, events, incidents, recovery runs) and a monitoring-resource / recovery-policy editor.
 custos 的 LuCI 界面：设置页、实时状态面板（健康、事件、事故、恢复运行）与监控资源/恢复策略编辑器。
EOF

chmod 755 "$R"/CONTROL/postinst "$R"/CONTROL/prerm
tar -czf "$R/control.tar.gz" -C "$R/CONTROL" ./control ./postinst ./prerm
tar --owner=0 --group=0 --numeric-owner -czf "$R/data.tar.gz" -C "$R/data" usr
echo "2.0" > "$R/debian-binary"
tar -czf "$OUT/luci-app-custos_${REL}_all.ipk" -C "$R" \
	./debian-binary ./control.tar.gz ./data.tar.gz
echo "built: $OUT/luci-app-custos_${REL}_all.ipk"

# ---- app-meta-custos (all) — the iStore-facing metadata package ------------
R="$STAGE/meta"
mkdir -p "$R/data/usr/lib/opkg/meta" "$R/data/www/luci-static/resources/app-icons" \
	"$R/CONTROL"
cp "$SRC/app-meta/custos.json" "$R/data/usr/lib/opkg/meta/custos.json"
cp assets/icons/custos.png "$R/data/www/luci-static/resources/app-icons/custos.png"

cat > "$R/CONTROL/postinst" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$R/CONTROL/prerm" <<'EOF'
#!/bin/sh
exit 0
EOF
SIZE=$(du -sk "$R/data" | cut -f1)
cat > "$R/CONTROL/control" <<EOF
Package: app-meta-custos
Version: $REL
Depends: luci-app-custos
Source: https://github.com/cuihairu/custos
SourceName: app-meta-custos
License: MIT
Section: admin
SourceDateEpoch: 1794931200
Maintainer: cuihairu <chuihairu@gmail.com>
Architecture: all
Installed-Size: $SIZE
Description: iStore app metadata for custos (icon and app manifest).
 custos 的 iStore 应用元数据（图标与应用清单）。
EOF

chmod 755 "$R"/CONTROL/postinst "$R"/CONTROL/prerm
tar -czf "$R/control.tar.gz" -C "$R/CONTROL" ./control ./postinst ./prerm
tar --owner=0 --group=0 --numeric-owner -czf "$R/data.tar.gz" -C "$R/data" usr www
echo "2.0" > "$R/debian-binary"
tar -czf "$OUT/app-meta-custos_${REL}_all.ipk" -C "$R" \
	./debian-binary ./control.tar.gz ./data.tar.gz
echo "built: $OUT/app-meta-custos_${REL}_all.ipk"
