<div align="center">

# aether

### See the invisible.

<p><strong>Local, live observability for Claude Code and Codex.</strong><br>
See context, cost, latency, tools, code changes, compactions, and agents in one terminal.</p>

<p>
  <a href="https://aether.haciensus.com"><strong>Website</strong></a>
  &nbsp;&middot;&nbsp;
  <a href="#quick-start"><strong>Quick start</strong></a>
  &nbsp;&middot;&nbsp;
  <a href="https://github.com/connectchiragg/aether/releases/latest"><strong>Releases</strong></a>
</p>

<p><a href="https://github.com/connectchiragg/aether/releases/latest"><img src="https://img.shields.io/github/v/release/connectchiragg/aether?style=flat-square&color=cf3f32" alt="Latest release"></a> <a href="LICENSE"><img src="https://img.shields.io/github/license/connectchiragg/aether?style=flat-square&color=e5b94b" alt="MIT license"></a> <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/built_with-Rust-cf3f32?style=flat-square" alt="Built with Rust"></a> <a href="#quick-start"><img src="https://img.shields.io/badge/platforms-macOS%20%7C%20Linux-d6d3d1?style=flat-square" alt="macOS and Linux"></a></p>

<br>

<a href="https://aether.haciensus.com"><img src="docs/assets/aether-launch.png" alt="Aether launch screen discovering local AI coding providers" width="100%"></a>

</div>

https://github.com/user-attachments/assets/c550f7cc-3017-4a40-a6e3-76684fa2aa2a

<div align="center">
  <sub>A silent product tour, captured in a standard terminal.</sub>
</div>

---

Aether turns the native local traces from Claude Code and Codex into a live terminal dashboard. It groups chats by project, keeps agents inside their parent turn, and updates while work is happening. No SDK, API key, hook installation, or hosted dashboard required.

## Quick Start

```bash
brew trust --formula connectchiragg/tap/aether
brew install connectchiragg/tap/aether
aether watch
```

Trust is a one-time Homebrew 6 requirement for third-party formulae. It applies only to Aether. Prebuilt bottles mean Aether does not compile locally or require Apple Command Line Tools.

Without Homebrew:

```bash
curl -fsSL https://raw.githubusercontent.com/connectchiragg/aether/master/install.sh | bash
aether watch
```

`aether watch` automatically discovers every supported provider already present on the machine.

## What You See

<img src="docs/assets/aether-metrics.png" alt="Aether synchronized terminal metrics dashboard" width="100%">

| View | What it shows |
|---|---|
| **Synchronized graphs** | Context, duration, estimated cost, tokens, turn complexity, and code diff across the same turns |
| **Input work** | What occupied the model input for the selected request |
| **Agent detail** | Which agents ran, their instructions, responses, tools, cost, and outcome |
| **Native telemetry** | Models, tools, searches, patches, compactions, cache use, and outcomes |
| **Organized sessions** | Provider-native titles grouped by project, with live and present states |

Move between turns once and every graph stays aligned, making spikes and correlations easy to inspect.

## Input Work

Each turn includes a left-to-right attribution tree:

```text
user request  ->  category  ->  named source
```

| Group | Includes |
|---|---|
| **Request** | User prompt |
| **History** | Previous context and the active compacted summary |
| **Execution** | Tools, MCPs, and agents |
| **Injected context** | Hooks, memory, documents, and knowledge bases |
| **Runtime** | Base instructions and provider-managed input |

Nodes show tokens, percentage, estimated cost, and duration. Provider totals are exact when emitted; lower-level attribution is deterministic and marked `~`. Raw prompts, tool arguments, schemas, memory, and document contents are never shown.

## Agents Stay With Their Work

<img src="docs/assets/aether-agents.png" alt="Aether showing nested agents inside their parent Codex turn" width="100%">

A collapsed turn shows how many agents ran; expand it to inspect each agent without losing the parent request.

## Provider Support

| Provider | Native data understood |
|---|---|
| **Claude Code** | Projects, titles, models, tools, cache usage, thinking, agents, hooks, memory, attachments, edits, and compactions |
| **Codex** | Projects, task titles, models, context windows, tools, reasoning, agents, hooks, memory, patches, attachments, and compactions |

Aether reads `~/.claude/projects/` and `~/.codex/sessions/`. Provider files remain the source of truth and are never modified.

## Navigation

| Key | Action |
|---|---|
| `Up` / `Down` | Move through sessions; scroll in the turn explorer |
| `Left` / `Right` | Move through providers or turns |
| `Enter` | Open the selected provider or session |
| `Esc` | Go back |
| `n` / `p` | Open the next or previous session |
| `h` / `Home`, `l` / `End` | Jump to the first or latest turn |
| `g`, number, `Enter` | Go to a specific turn |
| `j` / `k`, mouse, or trackpad | Scroll the whole page |
| `e` | Expand or collapse prompt, response, and agent details |
| `r` | Rename a session locally |
| `Ctrl-L` | Force a clean redraw |
| `q` or `Ctrl-C` | Quit |

## Accuracy

Known models are priced from the versioned catalog in [`src/model/pricing.json`](src/model/pricing.json). Cost is an API-equivalent estimate, not a provider invoice. Unknown models and incomplete pricing stay unavailable or `partial` instead of being guessed.

Turn complexity converts provider reasoning or thinking usage into a stable 0-100 comparison signal. It measures work, not answer quality.

## Local by Design

- No hosted telemetry backend
- No Claude or OpenAI API key
- No additional model calls
- No uploaded prompts, responses, files, or metrics
- No changes to provider transcripts

## Troubleshooting

| Problem | Fix |
|---|---|
| No sessions appear | Run Claude Code or Codex once so it creates a native session |
| Terminal looks incomplete after sleep or resize | Press `Ctrl-L`; Aether also detects resume gaps automatically |
| Cost is unavailable | The model ID or pricing entry is missing, so Aether leaves it unknown |

## Build and Remove

Build from source:

```bash
git clone https://github.com/connectchiragg/aether.git
cd aether
cargo build --release
./target/release/aether watch
```

Uninstall:

```bash
brew uninstall aether
```

For a direct installation:

```bash
curl -fsSL https://raw.githubusercontent.com/connectchiragg/aether/master/uninstall.sh | bash
```

## Contributing

Issues and pull requests are welcome, especially for new providers, native telemetry, pricing updates, parser fixtures, and terminal compatibility. Keep metrics deterministic and label estimates explicitly.

## License

[MIT](LICENSE)

<div align="center">
  <p><a href="https://aether.haciensus.com"><strong>aether.haciensus.com</strong></a></p>
  <sub>Built for people who want to understand what their agents are actually doing.</sub>
</div>
