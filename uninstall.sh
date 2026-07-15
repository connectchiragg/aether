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

# Remove the legacy Aether Claude skill
if [ -d "$HOME/.claude/skills/aether" ]; then
  rm -rf "$HOME/.claude/skills/aether"
  dim "  Removed skill"
fi

# Remove legacy Aether Claude hook scripts
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

# Remove legacy evaluator recaps
if [ -d "$HOME/.claude/.aether-recaps" ]; then
  rm -rf "$HOME/.claude/.aether-recaps"
  dim "  Removed recaps cache"
fi

# Remove legacy Aether-owned Claude metrics sidecars
for f in "$HOME"/.claude/threads/aether-metrics-*.jsonl; do
  if [ -f "$f" ]; then
    rm -f "$f"
  fi
done

# Remove UUID-named sidecars created by the legacy Claude skill. Native Claude
# transcripts live under ~/.claude/projects, not ~/.claude/threads.
THREADS="$HOME/.claude/threads"
if [ -d "$THREADS" ] && command -v python3 >/dev/null 2>&1; then
  python3 - "$THREADS" << 'PYEOF'
import json, pathlib, sys

threads = pathlib.Path(sys.argv[1])
for path in threads.glob("*.jsonl"):
    try:
        with path.open() as handle:
            first = json.loads(handle.readline())
        if (
            first.get("type") == "session_start"
            and first.get("sessionId") == path.stem
            and "ts" in first
        ):
            path.unlink()
    except Exception:
        pass
PYEOF
fi

# Remove legacy Aether commands from every Claude hook event while preserving other hooks
SETTINGS="$HOME/.claude/settings.json"
if [ -f "$SETTINGS" ] && command -v python3 >/dev/null 2>&1; then
  python3 - "$SETTINGS" << 'PYEOF'
import json, sys, os
path = sys.argv[1]
try:
    with open(path) as f:
        s = json.load(f)
    hooks = s.get("hooks", {})
    for event_name, entries in list(hooks.items()):
        cleaned_entries = []
        for entry in entries if isinstance(entries, list) else []:
            event_hooks = entry.get("hooks", [])
            event_hooks = [
                hook for hook in event_hooks
                if "aether" not in hook.get("command", "").lower()
            ]
            if event_hooks:
                entry["hooks"] = event_hooks
                cleaned_entries.append(entry)
        if cleaned_entries:
            hooks[event_name] = cleaned_entries
        else:
            hooks.pop(event_name, None)
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
