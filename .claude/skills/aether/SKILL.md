---
name: aether
description: |
  Toggle live agent observability and turn-level quality metrics.
  Enables/disables automatic logging of Agent tool calls and
  per-turn Haiku-scored metrics (friction, hallucination, confidence,
  acceptance, performance). Run /aether to toggle on or off.
allowed-tools:
  - Bash
---

# Aether — Live Agent Observability

When this skill is invoked, first check if aether is currently enabled:

```bash
if [ -f ~/.claude/hooks/aether-hook.py ] || [ -f ~/.claude/hooks/aether-metrics.py ]; then
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

First, restore or install the hooks:

```bash
mkdir -p ~/.claude/hooks

# Agent logging hook
if [ -f ~/.claude/hooks/aether-hook.py.off ]; then
  mv ~/.claude/hooks/aether-hook.py.off ~/.claude/hooks/aether-hook.py
fi

# Metrics hook
if [ -f ~/.claude/hooks/aether-metrics.py.off ]; then
  mv ~/.claude/hooks/aether-metrics.py.off ~/.claude/hooks/aether-metrics.py
elif [ ! -f ~/.claude/hooks/aether-metrics.py ]; then
  # Install from repo if available
  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  REPO_HOOK="$SCRIPT_DIR/../../hooks/aether-metrics.py"
  if [ -f "$REPO_HOOK" ]; then
    cp "$REPO_HOOK" ~/.claude/hooks/aether-metrics.py
    chmod +x ~/.claude/hooks/aether-metrics.py
  fi
fi
```

Then register the hooks in Claude Code settings if not already registered:

```bash
# Check if hooks are configured in settings
SETTINGS_FILE=~/.claude/settings.json
if [ -f "$SETTINGS_FILE" ]; then
  echo "Settings file exists — verify hooks are registered"
else
  echo "No settings file — hooks need manual registration"
fi
```

Print:

> **Aether enabled.** All agent calls and turn quality metrics will be logged.
>
> **Important:** Make sure `ANTHROPIC_API_KEY` is set in your environment for metrics scoring.
>
> Open a second terminal and run:
> ```
> aether watch
> ```
>
> Quality metrics (friction, hallucination, confidence, acceptance, performance)
> will appear on each new turn in the aether graph view.

Then STOP.
