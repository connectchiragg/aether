#!/bin/bash
set -euo pipefail

REPO="connectchiragg/aether"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
SKILL_DIR="$HOME/.claude/skills/aether"

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

if [ -w "$INSTALL_DIR" ]; then
  mv "${TMPDIR}/aether" "${INSTALL_DIR}/aether"
else
  sudo mv "${TMPDIR}/aether" "${INSTALL_DIR}/aether"
fi
chmod +x "${INSTALL_DIR}/aether"

# Install Claude Code skill
info "Installing Claude Code skill..."
mkdir -p "$SKILL_DIR"
cat > "${SKILL_DIR}/SKILL.md" << 'SKILL_EOF'
---
name: aether
description: |
  Toggle live agent observability. Enables/disables automatic logging
  of Agent tool calls to JSONL files that the aether viewer renders.
  Run /aether to toggle on or off.
allowed-tools:
  - Bash
---

# Aether — Live Agent Observability

When this skill is invoked, first check if aether is currently enabled:

```bash
if [ -f ~/.claude/hooks/aether-hook.py ]; then
  echo "AETHER_STATUS=enabled"
else
  echo "AETHER_STATUS=disabled"
fi
```

## If currently ENABLED → turn it OFF

```bash
mv ~/.claude/hooks/aether-hook.py ~/.claude/hooks/aether-hook.py.off
```

Print:

> **Aether disabled.** Agent calls will no longer be logged.
> Run `/aether` again to re-enable.

Then STOP. Do not proceed to the enable steps.

## If currently DISABLED → turn it ON

```bash
mv ~/.claude/hooks/aether-hook.py.off ~/.claude/hooks/aether-hook.py
```

Print:

> **Aether enabled.** All agent calls will be logged automatically.
>
> Open a second terminal and run:
> ```
> aether watch
> ```

Then STOP.
SKILL_EOF

# Install metrics hook
HOOKS_DIR="$HOME/.claude/hooks"
info "Installing metrics hook..."
mkdir -p "$HOOKS_DIR"
curl -fsSL "https://raw.githubusercontent.com/${REPO}/master/.claude/hooks/aether-metrics.py" \
  -o "${HOOKS_DIR}/aether-metrics.py.off" 2>/dev/null || true
chmod +x "${HOOKS_DIR}/aether-metrics.py.off" 2>/dev/null || true

echo ""
info "Installation complete!"
echo ""
dim "  Binary:  ${INSTALL_DIR}/aether"
dim "  Skill:   ${SKILL_DIR}/SKILL.md"
dim "  Hook:    ${HOOKS_DIR}/aether-metrics.py.off (inactive until /aether is run)"
echo ""
echo -e "  Run ${BOLD}aether watch${NC} to start observing Claude Code sessions."
echo -e "  Use ${BOLD}/aether${NC} in Claude Code to enable agent logging + quality metrics."
echo ""
