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
mkdir -p "$INSTALL_DIR" 2>/dev/null || sudo mkdir -p "$INSTALL_DIR"

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
  Toggle live agent observability and per-turn quality metrics.
  Run /aether to toggle on or off.
allowed-tools:
  - Bash
---

# Aether — Live Agent Observability

When this skill is invoked, first check if aether is currently enabled:

```bash
if [ -f ~/.claude/hooks/aether-metrics.py ]; then
  echo "AETHER_STATUS=enabled"
else
  echo "AETHER_STATUS=disabled"
fi
```

## If currently ENABLED → turn it OFF

```bash
[ -f ~/.claude/hooks/aether-hook.py ] && mv ~/.claude/hooks/aether-hook.py ~/.claude/hooks/aether-hook.py.off
[ -f ~/.claude/hooks/aether-metrics.py ] && mv ~/.claude/hooks/aether-metrics.py ~/.claude/hooks/aether-metrics.py.off
```

Print:

> **Aether disabled.** Agent logging and metrics scoring are off.
> Run `/aether` again to re-enable.

Then STOP. Do not proceed to the enable steps.

## If currently DISABLED → turn it ON

```bash
mkdir -p ~/.claude/hooks
[ -f ~/.claude/hooks/aether-hook.py.off ] && mv ~/.claude/hooks/aether-hook.py.off ~/.claude/hooks/aether-hook.py
[ -f ~/.claude/hooks/aether-metrics.py.off ] && mv ~/.claude/hooks/aether-metrics.py.off ~/.claude/hooks/aether-metrics.py
```

Print:

> **Aether enabled.** Per-turn quality metrics will be scored live.
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

# Register Stop hook in Claude Code settings
SETTINGS_FILE="$HOME/.claude/settings.json"
info "Registering hooks in Claude Code settings..."
if command -v python3 >/dev/null 2>&1; then
  python3 - "$SETTINGS_FILE" << 'PYEOF'
import json, sys, os
path = sys.argv[1]
settings = {}
if os.path.exists(path):
    with open(path) as f:
        try:
            settings = json.load(f)
        except json.JSONDecodeError:
            pass

hooks = settings.get("hooks", {})

# Add Stop hook if not already present
stop_hooks = hooks.get("Stop", [])
metrics_cmd = "python3 ~/.claude/hooks/aether-metrics.py"
already = any(
    metrics_cmd in h.get("command", "")
    for entry in stop_hooks
    for h in entry.get("hooks", [])
)
if not already:
    stop_hooks.append({
        "matcher": "",
        "hooks": [{"type": "command", "command": metrics_cmd}]
    })
    hooks["Stop"] = stop_hooks
    settings["hooks"] = hooks
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(settings, f, indent=2)
PYEOF
fi

echo ""
info "Installation complete!"
echo ""
dim "  Binary:    ${INSTALL_DIR}/aether"
dim "  Skill:     ${SKILL_DIR}/SKILL.md"
dim "  Hook:      ${HOOKS_DIR}/aether-metrics.py.off (inactive)"
dim "  Settings:  Stop hook registered"
echo ""
echo -e "  ${BOLD}Step 1:${NC} Run ${BOLD}aether watch${NC} in a terminal"
echo -e "  ${BOLD}Step 2:${NC} Type ${BOLD}/aether${NC} in Claude Code to enable metrics"
echo ""
