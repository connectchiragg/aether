#!/bin/bash
set -euo pipefail

REPO="connectchiragg/aether"
# Use /opt/homebrew/bin on Apple Silicon, /usr/local/bin otherwise
if [ -d "/opt/homebrew/bin" ]; then
  DEFAULT_DIR="/opt/homebrew/bin"
elif [ -d "/usr/local/bin" ]; then
  DEFAULT_DIR="/usr/local/bin"
else
  DEFAULT_DIR="$HOME/.local/bin"
fi
INSTALL_DIR="${INSTALL_DIR:-$DEFAULT_DIR}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
DIM='\033[2m'
BOLD='\033[1m'
NC='\033[0m'

info() { echo -e "${GREEN}${BOLD}==>${NC} $1"; }
dim() { echo -e "${DIM}$1${NC}"; }
err() { echo -e "${RED}error:${NC} $1" >&2; exit 1; }

# Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin) PLATFORM="apple-darwin" ;;
  Linux)  PLATFORM="unknown-linux-gnu" ;;
  *)      err "Unsupported OS: $OS" ;;
esac

case "$ARCH" in
  x86_64|amd64)  ARCH="x86_64" ;;
  arm64|aarch64) ARCH="aarch64" ;;
  *)             err "Unsupported architecture: $ARCH" ;;
esac

TARGET="${ARCH}-${PLATFORM}"

info "Detected platform: ${TARGET}"

# Get latest release
info "Fetching latest release..."
RELEASE_URL="https://api.github.com/repos/${REPO}/releases/latest"
RELEASE_JSON=$(curl -fsSL "$RELEASE_URL" 2>/dev/null) || err "Failed to fetch release info. Check https://github.com/${REPO}/releases"

TAG=$(echo "$RELEASE_JSON" | grep '"tag_name"' | head -1 | sed 's/.*: "//;s/".*//')
[ -z "$TAG" ] && err "Could not determine latest version"

info "Latest version: ${TAG}"

# Download binary
ASSET_NAME="aether-${TARGET}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET_NAME}"

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

info "Downloading ${ASSET_NAME}..."
curl -fsSL "$DOWNLOAD_URL" -o "${TMPDIR}/${ASSET_NAME}" || err "Failed to download binary for ${TARGET}. Check available assets at https://github.com/${REPO}/releases/tag/${TAG}"

info "Installing to ${INSTALL_DIR}/aether..."
tar -xzf "${TMPDIR}/${ASSET_NAME}" -C "$TMPDIR"
mkdir -p "$INSTALL_DIR" 2>/dev/null || sudo mkdir -p "$INSTALL_DIR"

if [ -w "$INSTALL_DIR" ]; then
  mv "${TMPDIR}/aether" "${INSTALL_DIR}/aether"
else
  sudo mv "${TMPDIR}/aether" "${INSTALL_DIR}/aether"
fi
chmod +x "${INSTALL_DIR}/aether"

echo ""
info "Installation complete!"
echo ""
dim "  Binary:    ${INSTALL_DIR}/aether"
echo ""
echo -e "  ${BOLD}Step 1:${NC} Run ${BOLD}aether setup claude${NC} or ${BOLD}aether setup codex${NC}"
echo -e "  ${BOLD}Step 2:${NC} Run ${BOLD}aether watch${NC}"
echo ""
