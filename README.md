# aether

See the invisible — live agent observability for Claude Code.

A terminal UI that watches Claude Code sessions in real-time, showing token usage, costs, sub-agent activity, per-turn quality metrics, and an interactive cost explorer.

## Quick Start

### 1. Install

**Homebrew (macOS/Linux):**

```bash
brew tap connectchiragg/tap
brew install aether
```

**Or via script:**

```bash
curl -fsSL https://raw.githubusercontent.com/connectchiragg/aether/master/install.sh | bash
```

Both install the `aether` binary and set up the `/aether` Claude Code skill.

### 2. Enable metrics (optional)

Inside any Claude Code session, type:

```
/aether
```

This enables per-turn quality scoring (friction, hallucination, confidence, acceptance, performance) powered by Haiku. Metrics are scored live as you work. Type `/aether` again to disable.

### 3. Watch

Open a second terminal:

```bash
aether watch
```

That's it. You'll see all your Claude Code sessions updating live.

## Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/connectchiragg/aether/master/uninstall.sh | bash
```

Removes the binary, skill, hooks, recaps cache, and cleans up settings.json.

## Install from source

```bash
cargo install --git https://github.com/connectchiragg/aether
```

Then set up the skill and hook:

```bash
# Skill
mkdir -p ~/.claude/skills/aether
curl -fsSL https://raw.githubusercontent.com/connectchiragg/aether/master/.claude/skills/aether/SKILL.md \
  -o ~/.claude/skills/aether/SKILL.md

# Metrics hook (installed inactive — /aether activates it)
mkdir -p ~/.claude/hooks
curl -fsSL https://raw.githubusercontent.com/connectchiragg/aether/master/.claude/hooks/aether-metrics.py \
  -o ~/.claude/hooks/aether-metrics.py.off
chmod +x ~/.claude/hooks/aether-metrics.py.off
```

Register the Stop hook in `~/.claude/settings.json`:

```json
{
  "hooks": {
    "Stop": [{
      "matcher": "",
      "hooks": [{"type": "command", "command": "python3 ~/.claude/hooks/aether-metrics.py"}]
    }]
  }
}
```

### Requirements

- macOS (arm64/x86) or Linux (x86/arm64)
- Claude Code CLI installed
- A terminal with Unicode support

## What you see

### Session List

Browse all detected sessions with names, costs, and token counts.

### Graph View

Interactive dot graph with a detail panel per turn showing:

- **Prompt and Response** — user prompt and assistant response (press `e` to expand)
- **Cost and Tokens** — per-turn and cumulative context
- **Sub-agents** — spawned agents with their request/response
- **Quality Metrics** — friction, hallucination, confidence, acceptance, performance (scored by Haiku)
- **Reasoning** — Haiku's assessment of the turn

Press `c` to switch the graph between cost, friction, hallucination, confidence, acceptance, and performance.

### Keybindings

**Session List**

| Key | Action |
|-----|--------|
| `Up/Down` | Navigate sessions |
| `Enter` | Open session |
| `r` | Rename session |
| `q` | Quit |

**Graph View**

| Key | Action |
|-----|--------|
| `Left/Right` | Navigate turns |
| `Up/Down` | Switch sessions |
| `h/l` | First/last turn |
| `g` | Go to turn number |
| `c` | Change graph (cost/friction/hallucination/confidence/acceptance/performance) |
| `+/-` | Zoom in/out graph |
| `e` | Expand/collapse content |
| `Esc` | Back to session list |
| `q` | Quit |

Mouse scroll works in the detail panel.

## How it works

**Session data** — Reads Claude Code's native JSONL session files from `~/.claude/projects/*/`. Parses session names, token usage, costs per model (Opus/Sonnet/Haiku), and sub-agent activity. All data updates live.

**Quality metrics** — When `/aether` is enabled, a Stop hook runs after each Claude response. It calls `claude -p --model haiku` to score the turn, then writes the scores back into the session JSONL as a `turn-metrics` event. A rolling recap chains context between turns so Haiku can evaluate each turn in context. No API key needed — uses your Claude Code subscription.

**Cost** — Metrics scoring costs ~$0.001 per turn via Haiku.
