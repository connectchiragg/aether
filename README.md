<div align="center">

# aether

### See the invisible.

<p><strong>Local, live observability for Claude Code and Codex.</strong><br>Understand cost, context, complexity, code changes, tools, compactions, and sub-agents without leaving your terminal.</p>

<p><a href="https://github.com/connectchiragg/aether/releases/latest"><img src="https://img.shields.io/github/v/release/connectchiragg/aether?style=flat-square&color=cf3f32" alt="Latest release"></a> <a href="LICENSE"><img src="https://img.shields.io/github/license/connectchiragg/aether?style=flat-square&color=e5b94b" alt="License"></a> <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/built_with-Rust-cf3f32?style=flat-square" alt="Rust"></a> <a href="#install"><img src="https://img.shields.io/badge/platforms-macOS%20%7C%20Linux-d6d3d1?style=flat-square" alt="Platforms"></a></p>

<br>

<img src="docs/assets/aether-launch.png" alt="Aether launch screen scanning local AI coding providers" width="100%">

</div>

https://github.com/user-attachments/assets/c550f7cc-3017-4a40-a6e3-76684fa2aa2a

<div align="center">
  <sub>46-second product tour. Silent, local, and running in a standard terminal.</sub>
</div>

---

Your coding agent already leaves a trail. Aether turns that trail into a live operational picture.

It reads the native session files already written by Claude Code and Codex, organizes chats by project, preserves parent-to-agent relationships, and renders synchronized telemetry in a fast terminal UI. No SDK instrumentation. No API keys. No cloud dashboard.

<img src="docs/assets/aether-metrics.png" alt="Aether synchronized metrics dashboard" width="100%">

## Install

Three commands take you from zero to a live dashboard:

```bash
brew install connectchiragg/tap/aether
aether setup claude && aether setup codex
aether watch
```

Enable only the provider you use:

```bash
aether setup claude
# or
aether setup codex
```

Without Homebrew:

```bash
curl -fsSL https://raw.githubusercontent.com/connectchiragg/aether/master/install.sh | bash
```

Setup only enables local discovery for the selected provider. Aether does **not** install hooks, modify transcripts, or make additional model calls.

## What Aether Shows

| Signal | What it tells you |
|---|---|
| **Context** | How much of the model window is occupied, including real compaction resets |
| **Duration** | Which turns are slow and how far they deviate from the session median |
| **Estimated cost** | API-equivalent token cost per turn and across the session |
| **Tokens** | Input, output, cache, and reasoning usage emitted by the provider |
| **Turn complexity** | A deterministic 0-100% view of reasoning effort |
| **Code diff** | Successful additions, removals, created files, and deleted files |
| **Agent topology** | Which parent turn spawned sub-agents and what each agent did |
| **Actions** | Tools, patches, searches, compactions, outcomes, and model metadata |

All six graph panels share the same turn range and selection. Move once and every signal stays aligned, making correlations visible instead of forcing you to compare separate dashboards.

## Trace Agents, Not Extra Chats

Sub-agents and hooks belong to the parent turn that created them. Aether keeps them there.

<img src="docs/assets/aether-agents.png" alt="Aether showing five nested agents within their parent Codex turn" width="100%">

The turn detail shows each agent's request, response, model, duration, token use, tools, and code impact. Nested work no longer pollutes the session list or hides behind a single aggregate number.

## Provider Support

### Claude Code

Aether reads Claude Code's native JSONL sessions from `~/.claude/projects/` and understands:

- Native titles and project identity
- Parent sessions and nested agent activity
- Models, completion state, and duration
- Input, output, cache, and thinking-response usage
- Context utilization and deterministic complexity
- Successful edits, tools, and outcomes

Repeated content blocks sharing a Claude message ID are counted once.

### Codex

Aether reads Codex rollouts from `~/.codex/sessions/` and native titles from the Codex session index. It understands:

- Native task titles and project identity
- Parent turns, sub-agents, and hook activity
- Model IDs and context windows
- Input, cached input, output, and reasoning tokens
- Duration, tools, patches, searches, and compactions
- Applied unified diffs and code-line impact

Provider cards update as supported tools appear on the machine. A live provider flickers; an enabled but idle provider stays solid grey; a hollow marker means it is not set up.

## Cost Estimates

Aether uses the versioned catalog in [`src/model/pricing.json`](src/model/pricing.json), including model aliases, context windows, cache semantics, date-effective prices, and long-context rules.

These are transparent, token-only, API-equivalent estimates. Local Codex rollouts do not expose the amount charged against a ChatGPT subscription or credits balance. Tool fees, regional pricing, subscription allocation, and unknown models are never guessed. Mixed sessions are labeled `partial`.

## How It Works

```text
Claude JSONL ─┐
              ├─> provider parsers ─> normalized sessions ─> live TUI
Codex JSONL ──┘               │
                              └─> local model pricing catalog
```

1. Aether discovers native provider session files.
2. Provider-specific parsers normalize turns, usage, actions, and relationships.
3. Nested records are attached to their native parent session and turn.
4. The watcher incrementally refreshes the TUI as files change.
5. Pricing and complexity are derived deterministically from emitted telemetry.

Everything remains on your machine.

## Navigation

### Providers and Sessions

| Key | Action |
|---|---|
| `Arrow keys` | Move through providers or sessions |
| `Enter` | Open the selected item |
| `r` | Rename a session locally |
| `Esc` | Go back |
| `q` | Quit |

### Dashboard

| Key | Action |
|---|---|
| `Left` / `Right` | Move all graphs across turns |
| `Up` / `Down` | Switch sessions |
| `h` / `l` | Jump to the first or latest turn |
| `g` | Go to a turn number |
| `+` / `-` | Zoom all timelines together |
| `e` | Expand or collapse turn content |
| Mouse / trackpad | Scroll the complete dashboard |

## Privacy By Construction

- Session data is read locally.
- Aether has no hosted backend.
- No provider API key is required.
- No prompt, response, or metric is uploaded.
- No additional LLM is called to generate telemetry.
- Provider transcripts are never modified.

## Uninstall

Remove the Homebrew package:

```bash
brew uninstall aether
```

Remove Aether configuration and any artifacts from legacy releases:

```bash
curl -fsSL https://raw.githubusercontent.com/connectchiragg/aether/master/uninstall.sh | bash
```

## Build From Source

```bash
git clone https://github.com/connectchiragg/aether.git
cd aether
cargo build --release
./target/release/aether watch
```

Run the test suite with:

```bash
cargo test
```

## Contributing

Issues and pull requests are welcome, especially for:

- Additional coding-agent providers
- New native telemetry fields
- Model pricing updates with official sources
- Parser fixtures for provider format changes
- Terminal compatibility and rendering improvements

Please keep new metrics deterministic and label estimates explicitly.

## License

[MIT](LICENSE)

<div align="center">
  <sub>Built for people who want to understand what their agents are actually doing.</sub>
</div>
