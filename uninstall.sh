#!/bin/bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
DIM='\033[2m'
BOLD='\033[1m'
NC='\033[0m'

info() { echo -e "${GREEN}${BOLD}==>${NC} $1"; }
dim() { echo -e "${DIM}$1${NC}"; }

info "Removing aether..."

# Remove binary
for dir in /opt/homebrew/bin /usr/local/bin "$HOME/.local/bin"; do
  if [ -f "$dir/aether" ]; then
    rm -f "$dir/aether" 2>/dev/null || sudo rm -f "$dir/aether"
    dim "  Removed $dir/aether"
  fi
done

# Remove skill
if [ -d "$HOME/.claude/skills/aether" ]; then
  rm -rf "$HOME/.claude/skills/aether"
  dim "  Removed skill"
fi

# Remove hooks
for f in aether-hook.py aether-hook.py.off aether-metrics.py aether-metrics.py.off; do
  if [ -f "$HOME/.claude/hooks/$f" ]; then
    rm -f "$HOME/.claude/hooks/$f"
    dim "  Removed $f"
  fi
done

# Remove provider configuration and persisted session names
if [ -d "$HOME/.config/aether" ]; then
  rm -rf "$HOME/.config/aether"
  dim "  Removed configuration"
fi

# Remove recaps
if [ -d "$HOME/.claude/.aether-recaps" ]; then
  rm -rf "$HOME/.claude/.aether-recaps"
  dim "  Removed recaps cache"
fi

# Remove Stop hook from settings.json
SETTINGS="$HOME/.claude/settings.json"
if [ -f "$SETTINGS" ] && command -v python3 >/dev/null 2>&1; then
  python3 - "$SETTINGS" << 'PYEOF'
import json, sys, os
path = sys.argv[1]
try:
    with open(path) as f:
        s = json.load(f)
    hooks = s.get("hooks", {})
    stop = hooks.get("Stop", [])
    stop = [e for e in stop if not any("aether" in h.get("command","") for h in e.get("hooks",[]))]
    if stop:
        hooks["Stop"] = stop
    else:
        hooks.pop("Stop", None)
    if hooks:
        s["hooks"] = hooks
    else:
        s.pop("hooks", None)
    with open(path, "w") as f:
        json.dump(s, f, indent=2)
except Exception:
    pass
PYEOF
  dim "  Cleaned settings.json"
fi

echo ""
info "Aether uninstalled."
echo ""
