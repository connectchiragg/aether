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
