# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

SMP (Strategic Memory Planner) is a local HTTP daemon (Axum, port 19800) that ingests AI session logs, runs a local LLM to extract structured decisions and phase signals, and serves a tiered memory API. Zero paid API calls — all inference is local.

## Commands

```bash
# Build
cargo build --release

# Run (config file is the only argument)
./target/release/smp smp.toml

# Run with debug logging
RUST_LOG=smp=debug ./target/release/smp smp.toml

# Run tests
cargo test

# Test ingest pipeline end-to-end
curl -s -X POST http://localhost:19800/ingest \
  -H "Content-Type: application/json" \
  -d '{"project":"myproject","source":"/path/to/session.jsonl","format":"jsonl"}'

# Read memory back
curl http://localhost:19800/memory/myproject/hot
curl http://localhost:19800/memory/myproject/state
curl http://localhost:19800/memory/myproject/brain
```

Convenience scripts `smp.sh` (build + run) and `dev.sh` are in the repo root.

## Architecture

The ingest pipeline is three sequential phases:

```
raw log file → Ingester (parse + filter) → Digest → Secretary (LLM) → SQLite state → exports
```

**Ingester** (`src/ingester/`) — pure Rust, no LLM. Parses JSONL/JSON/text logs into a `Digest` struct. Strips noise (thinking blocks, large tool results, repeated edits to same file). Token estimate: chars/4. The Digest's `to_text()` is what gets fed to the Secretary.

**Secretary** (`src/secretary/`) — trait-based LLM abstraction. Three backends, selected in `smp.toml`:
- `embedded` — llama-cpp-2 in-process, Metal GPU, `spawn_blocking` so Axum loop stays unblocked. Creates a fresh `LlamaContext` per inference call; uses greedy sampling (`candidates.sample_token_greedy()`). Prompt is wrapped in ChatML (`<|im_start|>system/user/assistant`) before tokenizing with `AddBos::Never`.
- `ollama` — HTTP to localhost:11434
- `openai-compat` — HTTP to any OpenAI-compatible endpoint

All backends implement `Secretary::extract(&prompt, &json_schema) -> Result<String>`. Extraction logic lives in `secretary/extract.rs` — `extract_decisions()` chunks the transcript and calls the Secretary per chunk; `detect_phase()` uses the tail of the transcript.

**State** (`src/memory/state.rs`) — rusqlite, WAL mode, one DB per project at `data/{project}/state.db`. Decisions are upserted (old version moves to `decision_history`). The DB is opened fresh per request via `AppState::open_project_db()` — no connection pool.

**Exports** — `memory/export.rs` writes `data/{project}/project_brain.md` and hot memory on every ingestion. The `/memory/{project}/hot` endpoint reads this file; `/brain` serves the markdown; `/state` queries SQLite directly.

**Events** (`src/events/`) — `tokio::broadcast` channel wrapped in `EventBus`. SSE endpoint at `/events/{project}` streams to subscribers. Events fire on new decisions, phase changes, and ingestion completion.

**AppState** is cloned per request (all fields are `Arc<T>`). Secretary and PromptLoader are loaded once at startup; SQLite connections are per-request.

## Key Files

| File | Role |
|------|------|
| `src/secretary/embedded.rs` | `run_inference()` — the full llama-cpp-2 inference loop |
| `src/secretary/extract.rs` | `extract_decisions()` / `detect_phase()` — chunking + JSON parsing |
| `src/memory/state.rs` | All SQLite reads/writes |
| `src/ingester/filter.rs` | Transcript compression logic |
| `src/api/ingest.rs` | The three-phase pipeline orchestration |
| `prompts/extract_decisions.txt` | Decision extraction prompt (`{chunk}` placeholder) |
| `prompts/detect_phase.txt` | Phase detection prompt |
| `smp.toml` | Runtime config — backend, model path, port, data dir |

## Config Notes

- `model_path` supports `~/` tilde expansion (handled in `config.rs`)
- `gpu_layers = 99` offloads all layers to Metal; reduce if you hit VRAM limits
- `context_size = 8192` — prompt + output must fit; the ingester chunks at ~6k tokens to stay safe
- `phase_confidence_threshold = 0.8` — phase changes are only written if the model reports ≥ this confidence

## Embedded Backend Gotchas

- Each inference creates a fresh `LlamaContext` (model is loaded once, held in `Arc<Mutex<EmbeddedInner>>`)
- Use a separate `LlamaBatch::new(1, 1)` for each generation step — do **not** `batch.clear()` and reuse the prompt batch; llama-cpp-2 leaves `n_tokens == 0` internally after clear, causing `Decode Error -1`
- The prompt must be ChatML-wrapped before tokenizing, or the model treats input as a conversation to continue rather than an instruction
- Use `candidates.sample_token_greedy()` — `sample_token(seed: u32)` does random sampling and hits EOS prematurely
