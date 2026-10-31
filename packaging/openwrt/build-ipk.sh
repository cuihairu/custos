#!/bin/sh
# Builds the OpenWrt .ipk package for custos (musl static binary).
# Usage: build-ipk.sh <path-to-musl-custos-binary> [output-dir]
set -eu

die() { printf 'build-ipk: %s\n' "$*" >&2; exit 1; }

BINARY="$1"
OUT="${2:-.}"
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# Derive the package architecture from the binary itself (not a hardcoded
# x86_64): `file` reports the ELF machine, mapped to opkg's architecture
# names. Only architectures we actually ship (x86_64, aarch64) are accepted.
ARCH=$(file -b "$BINARY" | sed -n 's/.*, \(x86-64\|aarch64\).*/\1/p' | head -1)
case "$ARCH" in
  x86-64) ARCH=x86_64 ;;
  aarch64) ARCH=aarch64 ;;
  *) die "unsupported binary architecture: $ARCH" ;;
esac
PKGDIR="$STAGE/pkg"

# Stage data
mkdir -p "$PKGDIR/etc/init.d" "$PKGDIR/etc/config" "$PKGDIR/etc/custos" \
         "$PKGDIR/usr/bin" "$PKGDIR/usr/lib/lua/luci/controller" \
         "$PKGDIR/usr/lib/lua/luci/model/cbi" "$PKGDIR/usr/lib/lua/luci/view/custos"
cp packaging/openwrt/etc/init.d/custos "$PKGDIR/etc/init.d/"
cp packaging/openwrt/etc/config/custos "$PKGDIR/etc/config/"
cp packaging/openwrt/etc/custos/custos.toml "$PKGDIR/etc/custos/"
cp packaging/openwrt/usr/lib/lua/luci/controller/custos.lua "$PKGDIR/usr/lib/lua/luci/controller/"
cp packaging/openwrt/usr/lib/lua/luci/model/cbi/custos.lua "$PKGDIR/usr/lib/lua/luci/model/cbi/"
cp packaging/openwrt/usr/lib/lua/luci/view/custos/status.htm "$PKGDIR/usr/lib/lua/luci/view/custos/"
cp "$BINARY" "$PKGDIR/usr/bin/custos"
chmod 755 "$PKGDIR/etc/init.d/custos" "$PKGDIR/usr/bin/custos"

# Stage control + postinst + prerm (postinst creates directories)
mkdir -p "$STAGE/CONTROL"
cp packaging/openwrt/CONTROL/control "$STAGE/CONTROL/control"
cp packaging/openwrt/CONTROL/postinst "$STAGE/CONTROL/postinst"
cp packaging/openwrt/CONTROL/prerm "$STAGE/CONTROL/prerm"

# Installed-Size must reflect the real payload (du in KiB, rounded up), not a
# hardcoded 15 MiB that drifts from the actual archive.
INSTALLED_SIZE=$(du -sk "$PKGDIR" | cut -f1)
sed -i "s/^Installed-Size: .*/Installed-Size: ${INSTALLED_SIZE}/" "$STAGE/CONTROL/control"

# Build control.tar.gz and data.tar.gz
tar -czf "$STAGE/control.tar.gz" -C "$STAGE/CONTROL" ./control ./postinst ./prerm
tar -czf "$STAGE/data.tar.gz" -C "$PKGDIR" \
  etc/config/custos etc/custos/custos.toml etc/init.d/custos \
  usr/bin/custos \
  usr/lib/lua/luci/controller/custos.lua \
  usr/lib/lua/luci/model/cbi/custos.lua \
  usr/lib/lua/luci/view/custos/status.htm

# Build .ipk (gzip tar wrapping the three files)
mkdir -p "$OUT"
echo "2.0" > "$STAGE/debian-binary"
tar -czf "$OUT/custos_${VERSION}-1_${ARCH}.ipk" \
    -C "$STAGE" ./debian-binary ./control.tar.gz ./data.tar.gz

echo "built: $OUT/custos_${VERSION}-1_${ARCH}.ipk"
