#!/bin/sh
# Custos one-click installer (nightly rolling build).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/cuihairu/custos/main/scripts/install.sh | sh
#
# - Detects architecture and OS (generic Linux vs OpenWrt/iStoreOS).
# - Downloads the matching nightly binary from GitHub Releases.
# - Installs to /usr/bin (OpenWrt) or /usr/local/bin (Linux).
# - Optionally registers a service (procd on OpenWrt, systemd on Linux)
#   when a config file already exists or --with-service is passed.
#
# This installs the rolling nightly prerelease, never a formal version.

set -eu

REPO="cuihairu/custos"
CHANNEL="nightly"
PREFIX="${CUSTOS_PREFIX:-/usr/local/bin}"
WITH_SERVICE="${CUSTOS_WITH_SERVICE:-auto}"
CONFIG_PATH="${CUSTOS_CONFIG:-/etc/custos/custos.toml}"
DATA_PATH="${CUSTOS_DB:-/etc/custos/custos.db}"

say()  { printf '%s\n' "$*"; }
die()  { printf 'install: %s\n' "$*" >&2; exit 1; }

# ── detect architecture ────────────────────────────────────────────────
arch=$(uname -m)
case "$arch" in
  x86_64|amd64)  asset_arch="x86_64"   ;;
  aarch64|arm64) asset_arch="aarch64"  ;;
  *) die "unsupported architecture: $arch (supported: x86_64, aarch64)" ;;
esac

# ── detect OS: OpenWrt/iStoreOS (procd, /etc/openwrt_release) vs generic ──
is_openwrt() {
  [ -f /etc/openwrt_release ] || [ -d /lib/functions/ubus ]
}

if is_openwrt; then
  PLATFORM="openwrt"
  PREFIX="/usr/bin"
  say "detected: OpenWrt/iStoreOS (${asset_arch})"
else
  PLATFORM="linux"
  say "detected: generic Linux (${asset_arch})"
fi

# ── find the nightly download URL ─────────────────────────────────────
fetch() {
  if command -v curl >/dev/null 2>&1; then curl -fsSL "$1"
  elif command -v wget >/dev/null 2>&1; then wget -qO- "$1"
  else die "need curl or wget"
  fi
}

download() {
  if command -v curl >/dev/null 2>&1; then curl -fsSL -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$1"
  else die "need curl or wget"
  fi
}

# Assets are named custos-linux-<arch>.tar.gz (glibc) and, for x86_64,
# custos-linux-musl-<arch>.tar.gz (fully static). Match the exact asset
# suffix so a glibc build is never conflated with the static musl build.
# OpenWrt/iStoreOS prefer the static musl tarball when one exists for this
# architecture; otherwise (e.g. aarch64) fall back to the glibc build.
find_asset() {
  fetch "https://api.github.com/repos/${REPO}/releases/tags/${CHANNEL}" \
    | grep -o "\"browser_download_url\": *\"[^\"]*$1\\.tar\\.gz\"" \
    | head -1 | grep -o 'https://[^"]*'
}

if [ "$PLATFORM" = "openwrt" ]; then
  asset=$(find_asset "custos-linux-musl-${asset_arch}")
  [ -z "$asset" ] && asset=$(find_asset "custos-linux-${asset_arch}")
else
  asset=$(find_asset "custos-linux-${asset_arch}")
fi

[ -n "$asset" ] || die "no nightly ${asset_arch} tarball found in ${REPO}/${CHANNEL}"

# ── download and extract ───────────────────────────────────────────────
tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

say "downloading: $asset"
download "$asset" "$tmpdir/custos.tar.gz" || die "download failed"

# Extract both the engine and the desktop tray binary (present since the
# nightly rolling build bundles `custos-tray` next to `custos`).
tar -xzf "$tmpdir/custos.tar.gz" -C "$tmpdir" custos custos-tray \
  || die "extract failed (binary not in tarball)"

# ── install ────────────────────────────────────────────────────────────
if [ "$PLATFORM" = "openwrt" ] && [ -n "${IPKG_INSTROOT:-}" ]; then
  # Building into an image: respect the staged root.
  PREFIX="${IPKG_INSTROOT%/}/usr/bin"
fi
mkdir -p "$PREFIX"
install_bin="$PREFIX/custos"
mv "$tmpdir/custos" "$install_bin"
chmod 755 "$install_bin"
say "installed: $install_bin"

# Tray binary: installed alongside the engine when present (never fatal;
# the engine is the primary artifact and tray is a desktop nicety).
if [ -f "$tmpdir/custos-tray" ]; then
  tray_bin="$PREFIX/custos-tray"
  mv "$tmpdir/custos-tray" "$tray_bin"
  chmod 755 "$tray_bin"
  say "installed: $tray_bin"
fi

# ── service registration ──────────────────────────────────────────────
register_service() {
  if [ "$PLATFORM" = "openwrt" ]; then
    cat > /etc/init.d/custos <<'INIT'
#!/bin/sh /etc/rc.common
START=99
USE_PROCD=1
PROG=/usr/bin/custos
CONF_TOML=/etc/custos/custos.toml
CONF_DB=/etc/custos/custos.db

start_service() {
  local enabled
  config_load custos
  config_get enabled main enabled '0'
  [ "$enabled" = "1" ] || return 0
  [ -f "$CONF_TOML" ] || {
    logger -t custos "no config at $CONF_TOML; not starting"
    return 1
  }
  procd_open_instance
  procd_set_param command "$PROG" start "$CONF_TOML" --db "$CONF_DB"
  procd_set_param respawn 3600 5 0
  procd_set_param stdout 1
  procd_set_param stderr 1
  procd_close_instance
}

stop_service() {
  service_stop "$PROG"
}

reload_service() {
  stop
  start
}
INIT
    chmod +x /etc/init.d/custos
    /etc/init.d/custos enable 2>/dev/null || true
    say "procd service registered (/etc/init.d/custos, enabled)"
  elif command -v systemctl >/dev/null 2>&1; then
    cat > /etc/systemd/system/custos.service <<'UNIT'
[Unit]
Description=Custos local self-healing engine
After=network.target

[Service]
ExecStart=PREFIX/custos start /etc/custos/custos.toml --db /etc/custos/custos.db
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
    sed -i "s|PREFIX/custos|${PREFIX}/custos|" /etc/systemd/system/custos.service
    systemctl daemon-reload
    systemctl enable custos 2>/dev/null || true
    say "systemd service registered (custos.service, enabled)"
  fi
}

case "$WITH_SERVICE" in
  auto)
    if [ -f "$CONFIG_PATH" ]; then
      register_service
    else
      say "service not registered (no config at $CONFIG_PATH); create it then re-run with CUSTOS_WITH_SERVICE=1"
    fi
    ;;
  1|yes|true)
    mkdir -p /etc/custos
    [ -f "$CONFIG_PATH" ] || printf 'version = 1\n' > "$CONFIG_PATH"
    register_service
    ;;
  0|no|false)
    say "service registration skipped"
    ;;
esac

say ""
say "custos installed from the $CHANNEL rolling build."
say "next steps:"
say "  1. write a config:  $CONFIG_PATH"
say "  2. start it:        $install_bin start $CONFIG_PATH --db $DATA_PATH"
say "  (or start the service registered above)"
