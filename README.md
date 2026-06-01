# aether

See the invisible -- live observability for coding agents.

A terminal UI that watches local agent session files in real time, showing sessions, turns, token usage, costs where known, sub-agent/tool activity, quality metrics where available, and an interactive graph view.

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

### 2. Enable Providers

```bash
aether setup claude
aether setup codex
```

Claude setup installs the Claude Code skill and metrics hook. Codex setup enables Aether's local Codex session watcher.

### 3. Watch

```bash
aether watch
```

`aether watch` opens a provider list. Choose a provider to browse its sessions.

You can also jump directly to a provider:

```bash
aether watch claude
aether watch codex
```

## Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/connectchiragg/aether/master/uninstall.sh | bash
```

Removes the binary, Claude Code skill/hooks, recaps cache, and cleans up Claude settings.

## Providers

### Claude Code

Reads Claude Code's local JSONL session files from `~/.claude/projects/*/` and Aether hook files from `~/.claude/threads/`. It parses session names, token usage, provider-aware model costs, sub-agent activity, and Aether `turn-metrics` events.

### Codex

Reads Codex rollout JSONL files from `~/.codex/sessions/**/*.jsonl`. It parses session metadata, user turns, assistant responses, tool/action events, model IDs, and token usage where present. Costs are calculated for recognized OpenAI/Codex models and shown as unknown for internal or unpriced model IDs.

## What You See

### Provider List

Browse enabled or available providers, their session counts, and recent activity.

### Session List

Browse sessions for the selected provider with names, source labels, provider-specific model costs where priced, token counts, and turn counts.

### Graph View

Interactive dot graph with a detail panel per turn showing:

- **Prompt and Response** -- user prompt and assistant response
- **Cost and Tokens** -- per-turn and cumulative context
- **Sub-agents / Tools** -- spawned agents, tool calls, and related output
- **Quality Metrics** -- friction, hallucination, confidence, acceptance, performance when available
- **Reasoning** -- available metric reasoning or recap when present

Press `c` to switch the graph between cost, friction, hallucination, confidence, acceptance, and performance.

## Keybindings

**Provider List**

| Key | Action |
|-----|--------|
| `Up/Down` | Navigate providers |
| `Enter` | Open provider |
| `q` | Quit |

**Session List**

| Key | Action |
|-----|--------|
| `Up/Down` | Navigate sessions |
| `Enter` | Open session |
| `r` | Rename session |
| `Esc` | Back to providers |
| `q` | Quit |

**Graph View**

| Key | Action |
|-----|--------|
| `Left/Right` | Navigate turns |
| `Up/Down` | Switch sessions |
| `h/l` | First/last turn |
| `g` | Go to turn number |
| `c` | Change graph |
| `+/-` | Zoom graph |
| `e` | Expand/collapse content |
| `Esc` | Back to session list |
| `q` | Quit |

Mouse scroll works in lists and detail panels.
