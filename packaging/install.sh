#!/bin/sh
# RelayHop installer for Linux (x86_64).
#
#   curl -fsSL https://raw.githubusercontent.com/enrell/relayhop/main/packaging/install.sh | sh
#
# Pinned version:
#   curl -fsSL .../install.sh | sh -s -- v0.2.0
#   # or: RELAYHOP_VERSION=v0.2.0 sh install.sh
#
# Installs to ~/.local/bin (no sudo), plus a desktop entry and icon.
# Override the download base for testing: RELAYHOP_BASE_URL=file:///path
set -eu

REPO="enrell/relayhop"
VERSION="${1:-${RELAYHOP_VERSION:-latest}}"
BASE_URL="${RELAYHOP_BASE_URL:-https://github.com/$REPO/releases}"
if [ "$VERSION" = "latest" ]; then
  URL_BASE="$BASE_URL/latest/download"
else
  case "$VERSION" in v*) ;; *) VERSION="v$VERSION";; esac
  URL_BASE="$BASE_URL/download/$VERSION"
fi

ARCH="$(uname -m)"
if [ "$ARCH" != "x86_64" ]; then
  echo "RelayHop: arquitetura não suportada: $ARCH (apenas x86_64 por enquanto)" >&2
  exit 1
fi

command -v curl >/dev/null || { echo "RelayHop: preciso de 'curl'" >&2; exit 1; }
command -v sha256sum >/dev/null || { echo "RelayHop: preciso de 'sha256sum'" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

# Test-only escape hatch (local file/http servers); never set in real use.
PROTO_ARGS="--proto =https"
if [ -n "${RELAYHOP_ALLOW_HTTP:-}" ]; then PROTO_ARGS=""; fi

echo "Baixando RelayHop $VERSION..."
# shellcheck disable=SC2086
curl -fsSL --retry 3 $PROTO_ARGS -o "$TMP/relayhop-linux-x64.tar.gz" "$URL_BASE/relayhop-linux-x64.tar.gz"
# shellcheck disable=SC2086
curl -fsSL --retry 3 $PROTO_ARGS -o "$TMP/relayhop-linux-x64.sha256" "$URL_BASE/relayhop-linux-x64.sha256"

echo "Verificando integridade..."
(cd "$TMP" && sha256sum -c relayhop-linux-x64.sha256)

tar -xzf "$TMP/relayhop-linux-x64.tar.gz" -C "$TMP"

BIN_DIR="$HOME/.local/bin"
mkdir -p "$BIN_DIR"
install -m 755 "$TMP/relayhop/relayhop" "$BIN_DIR/relayhop"

APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
mkdir -p "$APP_DIR" "$ICON_DIR"
if [ -f "$TMP/relayhop/assets/relayhop.svg" ]; then
  install -m 644 "$TMP/relayhop/assets/relayhop.svg" "$ICON_DIR/relayhop.svg"
fi
# Desktop entry pointing at the installed binary (tray StartupWMClass kept).
{
  echo "[Desktop Entry]"
  echo "Type=Application"
  echo "Name=RelayHop"
  echo "Comment=Um salto temporário pelo Tor para abrir o Discord"
  echo "Exec=$BIN_DIR/relayhop"
  echo "Icon=relayhop"
  echo "Terminal=false"
  echo "Categories=Network;"
  echo "StartupNotify=true"
  echo "StartupWMClass=relayhop"
} > "$APP_DIR/relayhop.desktop"
if command -v update-desktop-database >/dev/null; then
  update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi

echo
echo "RelayHop instalado em $BIN_DIR/relayhop"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "ATENÇÃO: $BIN_DIR não está no seu PATH. Adicione ao ~/.profile: export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac
echo "Execute com: relayhop"
