# aether

See the invisible -- live observability for coding agents.

A terminal UI that watches local agent session files in real time, showing sessions, turns, token usage, costs where known, sub-agent/tool activity, and synchronized metric timelines.

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

Setup enables Aether's local session watcher for each provider. Aether does not install provider hooks or make additional model calls.

### 3. Watch

```bash
aether watch
```

`aether watch` opens a provider list. Choose a provider to browse its sessions.

## Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/connectchiragg/aether/master/uninstall.sh | bash
```

Removes the binary and configuration. It also cleans up Aether's legacy Claude skill, hooks, recaps, sidecars, and settings entries from earlier releases.

## Providers

### Claude Code

Reads Claude Code's native local JSONL session files from `~/.claude/projects/*/`. It parses native titles, projects, token/cache usage, models, completion state, duration, context utilization, thinking effort, tool activity, and successful code edits. Repeated content-block records sharing a Claude message ID are counted once. Aether does not modify Claude's transcripts.

### Codex

Reads Codex rollout JSONL files from `~/.codex/sessions/**/*.jsonl` and native titles from the Codex session index. It parses projects, parent turns, nested sub-agent/hook activity, model IDs, input/cache/output/reasoning tokens, context windows, duration, completion state, tools, patches, searches, and compactions where Codex emits them.

### Cost Estimates

Aether computes token-only, API-equivalent USD estimates from the versioned catalog at `src/model/pricing.json`. The catalog records model aliases, context windows, cache semantics, date-effective prices, long-context rules, and official source URLs for current OpenAI and Anthropic models.

Codex local rollouts expose model and token usage, but not the actual per-turn amount charged to a ChatGPT subscription or credits balance. Aether therefore labels these values as estimates. Tool fees, subscription allocation, regional uplifts, and models missing from the catalog are not guessed. Sessions containing both priced and unpriced turns are marked `partial`.

## What You See

### Provider List

Browse enabled or available providers as large branded cards with their session counts and recent activity.

- A flickering solid circle means activity within the last five minutes.
- A grey solid circle means the provider is set up but idle.
- A hollow circle means the provider is not set up or has not been found yet.

Supported providers installed after Aether are discovered on the next scan. Run the corresponding `aether setup` command to enable its integration; an already-open watcher reloads that setup state automatically.

### Session List

Browse sessions grouped by project with native names, source labels, token-cost estimates where priced, token counts, and turn counts. Nested activity with a native parent relationship remains inside its parent session.

### Metrics Dashboard

Six synchronized metric panels in a 3 x 2 dashboard with a detail panel per turn showing:

- **Prompt and Response** -- user prompt and assistant response
- **Native Telemetry** -- model, outcome, duration, context utilization, cache ratio, turn complexity, tools, patches, searches, compactions, and code lines changed
- **Cost and Tokens** -- per-turn token-cost estimate and cumulative context
- **Sub-agents / Tools** -- spawned agents, tool calls, and related output

Context, duration, cost estimate, tokens, turn complexity, and code diff are visible together. Every panel shares the same turn range, selection, and zoom so `Left` and `Right` move the complete dashboard together. Context uses a fixed 0-100% scale and combines Claude's native input/cache buckets with the cataloged model window when Claude Code omits the window itself. Native request samples preserve each compaction's pre-reset and post-reset levels, highlighted with a yellow `▼`, while the regular turn dot remains the final KPI. Complexity is a deterministic 0-100% view capped at 16,000 effort tokens: exact provider reasoning tokens are preferred, with Claude thinking-response output used as a labeled upper-bound proxy when its exact breakdown is absent. Code diff counts successful Claude edits and applied Codex unified-diff additions plus removals. Missing native fields are shown as `not emitted`.

## Keybindings

**Provider List**

| Key | Action |
|-----|--------|
| `Left/Right` or `Up/Down` | Navigate providers |
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
| `+/-` | Zoom all metric timelines |
| `e` | Expand/collapse content |
| `Esc` | Back to session list |
| `q` | Quit |

Mouse scroll works in lists and detail panels.
