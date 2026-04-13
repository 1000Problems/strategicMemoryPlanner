# SEP MVP — Implementation Plan

## Prerequisites

```bash
# Install Ollama (only needed if using Ollama backend, not for embedded)
# brew install ollama

# Download a GGUF model for the embedded backend
# mkdir -p models
# Download from: https://huggingface.co/Qwen/Qwen2.5-7B-Instruct-GGUF
# File: qwen2.5-7b-instruct-q5_k_m.gguf (~5GB)

# Rust toolchain (stable)
rustup update stable
```

---

## Build Order

### Phase 1: Skeleton (day 1)

**Goal:** Axum server starts, /health responds, config loads.

```
cargo init sep
```

**Files:**

`Cargo.toml`
```toml
[package]
name = "sep"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
rusqlite = { version = "0.32", features = ["bundled"] }
llama-cpp-2 = { version = "0.1", features = ["metal"] }
anyhow = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
async-trait = "0.1"
tokio-stream = "0.1"
```

`src/main.rs` — CLI arg parsing (port, config path), Axum router, shared AppState.

`src/config.rs` — Load `sep.toml`, parse secretary backend choice, model path, db path, port.

`src/api/system.rs` — `GET /health` returns status + backend info.

`src/db/mod.rs` — Open/create SQLite DB, run migrations.

`src/db/migrations/001_initial.sql` — Create tables (decisions, blockers, open_questions, phase_log, decision_history, ingestion_log).

**Test:** `cargo run -- --config sep.toml` → server starts → `curl localhost:19800/health` returns JSON.

---

### Phase 2: Secretary Trait + Embedded Backend (day 1-2)

**Goal:** Load a GGUF model, send a prompt, get JSON back with grammar constraints.

`src/secretary/mod.rs` — Define the `Secretary` trait:
```rust
#[async_trait]
pub trait Secretary: Send + Sync {
    /// Run extraction. json_schema constrains output via GBNF grammar.
    async fn extract(&self, prompt: &str, json_schema: &str) -> Result<String>;
    fn name(&self) -> &str;
}
```

`src/secretary/embedded.rs` — Implement `EmbeddedSecretary`:
- On `new()`: load GGUF model from config path with Metal GPU layers
- `extract()`: runs in `spawn_blocking`, builds prompt tokens, applies GBNF grammar from json_schema, samples until done, returns string
- Model stays loaded in memory (wrapped in `Arc<Mutex<...>>`)

`src/secretary/prompts.rs` — Load prompt templates from `prompts/` directory. Interpolate `{chunk}` placeholder with transcript text.

**Test:** Unit test that loads model, sends a hardcoded transcript snippet, and gets valid JSON decisions back.

---

### Phase 3: Ingester (day 2)

**Goal:** Parse a Claude Code JSONL session log into a cleaned digest.

`src/ingester/mod.rs` — Public API: `ingest(path) -> Digest`.

`src/ingester/parser.rs` — Stream-parse Claude Code JSONL:
- Read line by line (serde_json::StreamDeserializer)
- Extract: role, content, tool_use name/input, tool_result content
- Skip: system messages, thinking blocks, retry attempts

`src/ingester/filter.rs` — Compress the parsed messages:
- Collapse consecutive edits to same file into one entry
- Truncate tool results over N tokens (configurable, default 500)
- Deduplicate repeated assistant messages (retries)
- Strip markdown formatting noise
- Output: `Digest { messages: Vec<DigestMessage>, token_estimate: usize }`

**Test:** Feed a real Claude Code JSONL file, verify output is <20% of input token count, verify no thinking blocks leak through.

---

### Phase 4: Decision Extraction Pipeline (day 2-3)

**Goal:** Ingest a log → extract decisions → store in SQLite → generate hot memory.

`src/secretary/extract.rs` — The extraction coordinator:
- Takes a `Digest` and a `&dyn Secretary`
- Chunks the digest if it exceeds model context (segment by conversation turns)
- Calls `secretary.extract()` with decision prompt + JSON schema
- Parses the JSON response into `Vec<Decision>`
- Deduplicates against existing decisions in DB (fuzzy match on domain + decision text)

`src/memory/state.rs` — SQLite operations:
- `upsert_decision()` — insert or update, move old version to history
- `get_decisions(project)` — all active decisions
- `get_decisions_by_domain(project, domain)`

`src/memory/hot.rs` — Generate hot memory:
- Query active decisions, blockers, questions, current phase
- Format as shorthand markdown under 500 tokens
- Write to memory (return as string, also cache in AppState)

`src/memory/export.rs` — Generate `project_brain.md`:
- Same data as hot memory but more complete
- Write to `data/{project}/project_brain.md`

**Test:** End-to-end: JSONL file → ingest → extract → verify decisions in SQLite → verify hot memory < 500 tokens.

---

### Phase 5: API + Events (day 3)

**Goal:** Full API surface, SSE event stream.

`src/api/ingest.rs`:
- `POST /ingest` — accepts { project, source, format }, spawns background job, returns job_id
- `GET /ingest/{job_id}` — returns job status

`src/api/memory.rs`:
- `GET /memory/{project}/hot` — returns hot memory markdown
- `GET /memory/{project}/state` — returns full state JSON
- `GET /memory/{project}/state/decisions` — filtered
- `GET /memory/{project}/brain` — returns project_brain.md

`src/events/emitter.rs`:
- `tokio::sync::broadcast` channel
- Events: `StateUpdated`, `DecisionNew`, `DecisionChanged`, `PhaseChanged`, `IngestionComplete`

`src/events/sse.rs`:
- `GET /events/{project}` — SSE stream using `axum::response::Sse`
- Filter events by project

**Test:** Start server, POST /ingest with real log, verify SSE receives `decision.new` events, GET /memory/*/hot returns valid markdown.

---

### Phase 6: Phase Detection (day 3-4)

**Goal:** Secretary detects when a session transitions between phases.

Add phase detection prompt to `prompts/detect_phase.txt`.

Add to extraction pipeline: after decision extraction, run phase detection on the same digest. If confidence > 0.8 and phase differs from current, update `phase_log` in SQLite and emit `phase.changed` event.

**Test:** Feed a transcript that clearly ends with "okay let's build this" → verify phase changes to "ready" → verify SSE emits event.

---

## MVP Deliverable

After Phase 6, SEP can:

1. Accept a Claude Code session log via `POST /ingest`
2. Parse it down to ~10-20% of original tokens (no LLM cost)
3. Extract decisions with rationale using a local GGUF model (no API cost)
4. Store decisions in SQLite with version history
5. Serve hot memory (<500 tokens) via `GET /memory/{project}/hot`
6. Export `project_brain.md` for human inspection
7. Detect phase transitions and emit SSE events
8. All of this with zero external dependencies beyond a GGUF model file

**pwork integration surface:** Subscribe to `GET /events/{project}`, react to `phase.changed`. Pull `GET /memory/{project}/hot` when spawning a new session.

---

## Config File (sep.toml)

```toml
[server]
port = 19800
data_dir = "./data"

[secretary]
backend = "embedded"
prompts_dir = "./prompts"

[secretary.embedded]
model_path = "./models/qwen2.5-7b-instruct-q5_k_m.gguf"
gpu_layers = 99
context_size = 8192

[ingester]
max_tool_result_tokens = 500
collapse_repeated_edits = true

[extraction]
phase_confidence_threshold = 0.8
```
