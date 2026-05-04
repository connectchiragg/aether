# aether

See the invisible — live agent observability for Claude Code.

A terminal UI that watches Claude Code sessions in real-time, showing token usage, costs, sub-agent activity, and an interactive turn-by-turn cost explorer.

## Install

```bash
cargo install --path .
```

Or build manually:

```bash
cargo build --release
# Binary at target/release/aether
```

### Requirements

- Rust 1.70+
- A terminal with Unicode/braille support

## Usage

### Watch live sessions

```bash
aether watch
```

Scans `~/.claude/projects/` for Claude Code session files and displays them in real-time. Shows the 50 most recent sessions sorted by last modified time.

### Watch a specific directory

```bash
aether watch --dir /path/to/threads
```

### Demo mode

```bash
aether
```

Runs a scripted demo with mock agent activity.

## Views

### Session List

Browse all detected sessions with names, costs, and token counts.

| Key | Action |
|-----|--------|
| `Up/Down` | Navigate sessions |
| `Enter` | Open session graph |
| `r` | Rename session |
| `q` | Quit |

### Graph (Cost Explorer)

Interactive dot graph showing cost per turn, with a detail panel for the selected turn including sub-agent request/response chat.

| Key | Action |
|-----|--------|
| `Left/Right` | Navigate turns |
| `Up/Down` | Switch sessions |
| `h` | Jump to first turn |
| `l` | Jump to latest turn |
| `g` | Go to turn number |
| `Esc` | Back to session list |
| `q` | Quit |

Mouse scroll works in the detail panel.

## Claude Code Skill

To add the `/aether` toggle skill to Claude Code, copy the skill file:

```bash
mkdir -p ~/.claude/skills/aether
cp .claude/skills/aether/SKILL.md ~/.claude/skills/aether/SKILL.md
```

Then use `/aether` in Claude Code to toggle agent call logging on/off.

## How it works

Aether reads Claude Code's native JSONL session files from `~/.claude/projects/*/`. It parses:

- **Session names** from `custom-title`, `ai-title`, or the first user prompt
- **Token usage** from assistant message `usage` fields
- **Costs** calculated per model (Opus, Sonnet, Haiku)
- **Sub-agents** from `<session-id>/subagents/` directories

All data updates live — new turns, costs, and sub-agent activity appear as they happen without reloading.
