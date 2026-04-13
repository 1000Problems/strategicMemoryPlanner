# SEP — Strategic Execution Planner

## Design Document v0.3 — April 2026

---

## The Problem

AI coding sessions produce massive transcripts full of decisions, context, and reasoning — but most of it is noise for the next session. There's no cheap, structured way to extract and maintain the *meaning* from those sessions without spending expensive API tokens on summarization.

SEP solves this by running a local LLM as a "Secretary" that ingests raw session logs, extracts structured meaning, and maintains a tiered memory system. **SEP is purely a memory management daemon.** It doesn't know what model made the decisions or what model will act on them. It just keeps the project brain current.

---

## Core Principles

1. **SEP is a memory engine.** It ingests, extracts, stores, and serves project state. Nothing else.
2. **SEP is standalone.** It knows nothing about pwork, Claude Code, Opus, or Sonnet. Any tool can use its API.
3. **Zero Anthropic tokens.** All extraction and maintenance runs on a local LLM. SEP never calls a paid API.
4. **State over history.** SEP tracks *current truth*, not conversation logs. Logs are input, state is output.
5. **Density over prose.** Every output is optimized for minimum tokens with maximum signal.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────┐
│                    Consumers                         │
│         (pwork, CLI, future tools)                   │
│                                                      │
│  Read memory, push logs, subscribe to state changes  │
└──────────────┬──────────────────────────┬───────────┘
               │ HTTP API                  │ SSE / Webhooks
               ▼                           ▲
┌─────────────────────────────────────────────────────┐
│                   SEP Daemon                         │
│                (Rust / Axum)                          │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌───────────────────┐  │
│  │ Ingester │→ │ Secretary│→ │  Memory Manager   │  │
│  │          │  │ (Ollama) │  │                   │  │
│  │ Parses   │  │ Extracts │  │ Hot / State / Deep│  │
│  │ raw logs │  │ meaning  │  │ tiers             │  │
│  └──────────┘  └──────────┘  └─────────┬─────────┘  │
│                                        │             │
│  ┌─────────────────────────────────────┐│            │
│  │          Event Emitter              ││            │
│  │  Notifies consumers of state changes││            │
│  └─────────────────────────────────────┘│            │
│                                         ▼            │
│  ┌──────────────────────────────────────────────┐    │
│  │              SQLite + Markdown                │    │
│  │  (source of truth + human-readable export)    │    │
│  └──────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────┐
│       Ollama Server          │
│  Model: Qwen2.5 14B Instruct│
│  localhost:11434             │
└─────────────────────────────┘
```

---

## Components

### 1. Ingester

**Input:** Raw session logs (JSONL, markdown transcripts, or plain text).

**Job:** Parse and clean the raw input — extract assistant messages, tool calls and results, user directives. Strip thinking blocks, retry noise, system prompts, redundant content.

**Output:** A cleaned "transcript digest" — roughly 10-20% of the raw token count.

**Implementation:**
- Rust-native JSON parsing (serde_json streaming)
- No LLM needed — pure parsing and filtering
- Configurable filters: skip tool results over N tokens, collapse repeated edits to same file, deduplicate
- Format-agnostic: accepts Claude Code JSONL, plain text transcripts, or structured markdown

### 2. Secretary (Local LLM Layer)

**Input:** Cleaned transcript digest from the Ingester.

**Job:** Extract structured meaning using a local LLM. Three extraction modes:

| Mode | Output | Purpose |
|------|--------|---------|
| **Decisions** | `{decision, rationale, alternatives_rejected, domain, files_affected}` | What was decided and why |
| **State Delta** | `{what_changed, new_blockers, resolved_blockers, open_questions, phase_signals}` | What's different now |
| **Synthesis** | `{summary, key_patterns, lessons}` | Deep memory generation (scheduled/on-demand) |

#### Backend Abstraction

SEP doesn't call any LLM runtime directly. All inference goes through a trait:

```rust
#[async_trait]
pub trait Secretary: Send + Sync {
    async fn extract(&self, prompt: &str, json_schema: &str) -> Result<String>;
    fn name(&self) -> &str;
}
```

Three backends ship with SEP. Users pick one in config:

| Backend | Crate | How it works | Best for |
|---------|-------|-------------|----------|
| **Embedded** (default) | `llama-cpp-2` | Loads GGUF model in-process, Metal GPU accel | Zero-dependency installs, offline use |
| **Ollama** | `reqwest` | HTTP to localhost:11434 | Users with existing Ollama setup |
| **OpenAI-compat** | `reqwest` | HTTP to any OpenAI-compatible API | LM Studio, llama-server, vLLM, LocalAI, etc. |

Config:
```toml
[secretary]
backend = "embedded"           # or "ollama" or "openai-compat"

[secretary.embedded]
model_path = "./models/qwen2.5-7b-instruct-q5_k_m.gguf"
gpu_layers = 99                # offload all layers to Metal GPU

[secretary.ollama]
url = "http://localhost:11434"
model = "qwen2.5:7b-instruct"

[secretary.openai_compat]
url = "http://localhost:1234/v1"
model = "local-model"
```

#### Default Model: Qwen2.5 7B Instruct

| Property | Value |
|----------|-------|
| Format | GGUF (Q5_K_M quantization) |
| Size | ~5 GB on disk, ~5-6 GB VRAM |
| Speed | 30-50 tok/s embedded, 50-70 tok/s via Ollama (Apple Silicon) |
| JSON output | Excellent — supports GBNF grammar-constrained sampling via `json_schema_to_grammar()` |
| Swappable | Any GGUF model works — change path in config |

Users can use any model that fits their hardware. A 7B on a laptop, a 70B on a workstation. SEP doesn't care.

#### Embedded Backend Details

- Crate: `llama-cpp-2 v0.1.144` (vendors llama.cpp, compiles via CMake + bindgen)
- Feature flag: `metal` for Apple Silicon GPU acceleration
- Inference is blocking → runs in `tokio::task::spawn_blocking()` to not block the Axum event loop
- JSON output enforced via GBNF grammar generated from the extraction schema
- Model loads once on startup, stays in memory

#### Future: Fine-Tuned Specialist Model

Because SEP's extraction task is narrow and predictable (always transcript → JSON, same schema), a fine-tuned 1-3B model would outperform a general-purpose 7B while running 2-3x faster. The path:
1. Generate 200-500 training pairs using Claude (transcript segment → expected JSON)
2. Fine-tune via unsloth/QLoRA
3. Export to GGUF
4. Drop into `model_path` config — zero code changes

This is a v2 optimization. The trait abstraction means we never touch the rest of SEP.

#### Prompt Strategy

SEP ships with extraction prompts in `prompts/` (~200 tokens each). Terse, structured, JSON-only output:

```
SYSTEM: You extract structured data from coding session transcripts.
Output ONLY valid JSON. Use technical shorthand. No prose.

USER: Extract decisions from this transcript segment.
Format: [{"decision":"...","rationale":"...","domain":"...","files":["..."]}]

<transcript>
{chunk}
</transcript>
```

JSON schema constraint (GBNF) ensures output is always valid — no parsing failures from malformed LLM output.

### 3. Memory Manager

Three tiers, each with a clear role and token budget:

#### Tier 1: Hot Memory

- **Budget:** < 500 tokens
- **Contents:** Active project phase, current blockers, most recent decisions, pointers to state memory
- **Format:** Shorthand bullet points
- **Updated:** After every ingestion cycle
- **Purpose:** A consumer can grab this and inject it anywhere — CLAUDE.md, system prompt, task brief. SEP doesn't care how it's used.

Example:
```markdown
## Project: pwork
- PHASE: design (WebSocket reconnection)
- DECIDED: Exponential backoff w/ jitter, max 30s (not linear)
- DECIDED: Status bar shows reconnection state
- BLOCKER: None
- OPEN: Should reconnect trigger full state resync or delta?
- FILES: src/ws/client.rs, src/ws/types.rs
- UPDATED: 2026-04-13T14:30:00
```

#### Tier 2: State Memory — SQLite

- **Role:** Source of truth for current project state
- **Logic:** Stateful upsert — when a decision changes, old version moves to history, new version becomes current
- **Temporal queries:** "What was the auth design before Tuesday's refactor?"

Core tables:
```sql
decisions (
  id TEXT PRIMARY KEY,
  project TEXT NOT NULL,
  domain TEXT,           -- e.g. "auth", "ws", "ui"
  decision TEXT NOT NULL,
  rationale TEXT,
  files TEXT,            -- JSON array
  created_at TEXT,
  updated_at TEXT
)

blockers (
  id TEXT PRIMARY KEY,
  project TEXT NOT NULL,
  description TEXT,
  status TEXT DEFAULT 'active',  -- active | resolved
  created_at TEXT,
  resolved_at TEXT
)

open_questions (
  id TEXT PRIMARY KEY,
  project TEXT NOT NULL,
  question TEXT,
  context TEXT,
  status TEXT DEFAULT 'open',  -- open | answered
  answer TEXT,
  created_at TEXT,
  answered_at TEXT
)

phase_log (
  id TEXT PRIMARY KEY,
  project TEXT NOT NULL,
  phase TEXT NOT NULL,    -- e.g. "design", "implementation", "review"
  domain TEXT,            -- what area is in this phase
  started_at TEXT,
  ended_at TEXT
)

-- History (old versions of overwritten state)
decision_history (
  id TEXT PRIMARY KEY,
  decision_id TEXT,
  old_decision TEXT,
  old_rationale TEXT,
  superseded_at TEXT
)

-- Ingestion tracking
ingestion_log (
  id TEXT PRIMARY KEY,
  project TEXT,
  source_path TEXT,
  digest_tokens INTEGER,
  extractions_run TEXT,  -- JSON array of modes run
  processed_at TEXT
)
```

#### Tier 3: Deep Memory — Markdown Archive

- **Role:** Historical "why" archive, long-term patterns, lessons learned
- **Format:** Markdown files organized by project/date
- **Access:** On-demand or scheduled synthesis
- **Generated by:** Secretary running a "deep synthesis" pass
- **Content:** Architecture evolution, recurring patterns, cross-session insights

#### Markdown Export

State Memory exports a human-readable `project_brain.md` on every update:
```markdown
# Project Brain — pwork
Generated: 2026-04-13T14:30:00

## Current Phase
- design: WebSocket reconnection

## Active Decisions
- WS: exponential backoff w/ jitter, max 30s
- Auth: JWT in httpOnly cookie, refresh via /token/refresh
- UI: CSS vars for theming, no Tailwind

## Blockers
- None

## Open Questions
- Should WS reconnect trigger full state resync or delta?
```

### 4. Event Emitter

**This is how consumers like pwork stay informed without polling.**

SEP emits events when state changes. A consumer subscribes and reacts however it wants — SEP doesn't prescribe what happens next.

**Events:**

| Event | Payload | Example consumer reaction |
|-------|---------|--------------------------|
| `state.updated` | `{project, changes: [...]}` | Refresh UI |
| `decision.new` | `{project, decision}` | Log to timeline |
| `decision.changed` | `{project, old, new}` | Alert user |
| `blocker.added` | `{project, blocker}` | Show warning |
| `blocker.resolved` | `{project, blocker}` | Clear warning |
| `phase.changed` | `{project, domain, old_phase, new_phase}` | **pwork could detect "design → ready" and know execution can begin** |
| `question.new` | `{project, question}` | Prompt user |
| `question.answered` | `{project, question, answer}` | Update context |
| `ingestion.complete` | `{project, job_id, stats}` | Confirm processing |

**Delivery mechanisms:**
- **SSE (Server-Sent Events):** `GET /events/{project}` — long-lived connection, consumer gets real-time pushes. Lightweight, HTTP-native.
- **Webhook (optional):** Register a callback URL, SEP POSTs events to it.

**The `phase.changed` event is the key integration point.** The Secretary extracts phase signals from transcripts — when the conversation shifts from design discussion to "okay, let's build this", the Secretary detects that and updates the phase. SEP emits `phase.changed`, and pwork (or any consumer) decides what to do.

Phase detection prompt:
```
SYSTEM: Analyze this transcript segment and determine the project phase.
Phases: "exploring", "design", "ready", "blocked", "review", "done"
Output: {"domain":"...","phase":"...","confidence":0.0-1.0,"signal":"quote from transcript"}
Only output a phase change if confidence > 0.8.
```

---

## API Surface

SEP exposes a local HTTP API (Axum, default port 19800):

### Ingestion
```
POST /ingest
  Body: { "project": "pwork", "source": "/path/to/session.jsonl", "format": "claude_jsonl|text|markdown" }
  → Triggers ingestion + extraction pipeline
  → Returns { "job_id": "..." }

GET  /ingest/{job_id}
  → Job status and stats
```

### Memory — Read
```
GET  /memory/{project}/hot
  → Hot context (<500 tokens, shorthand markdown)

GET  /memory/{project}/state
  → Full state as JSON (decisions, blockers, questions, phases)

GET  /memory/{project}/state/decisions
GET  /memory/{project}/state/decisions?domain=auth
GET  /memory/{project}/state/blockers
GET  /memory/{project}/state/blockers?status=active
GET  /memory/{project}/state/questions
GET  /memory/{project}/state/questions?status=open
GET  /memory/{project}/state/phases

GET  /memory/{project}/brain
  → project_brain.md content

GET  /memory/{project}/deep?query={search_term}
  → Full-text search across deep memory archive

GET  /memory/{project}/history/decisions/{decision_id}
  → Version history for a specific decision
```

### Memory — Write
```
POST /memory/{project}/decisions
  Body: { "domain": "ws", "decision": "...", "rationale": "..." }
  → Manually add/update a decision (bypass ingestion)

POST /memory/{project}/blockers
  Body: { "description": "..." }

POST /memory/{project}/questions
  Body: { "question": "...", "context": "..." }

PATCH /memory/{project}/blockers/{id}
  Body: { "status": "resolved" }

PATCH /memory/{project}/questions/{id}
  Body: { "status": "answered", "answer": "..." }

PATCH /memory/{project}/phases
  Body: { "domain": "ws", "phase": "ready" }
  → Manually set phase (override Secretary detection)
```

### Events
```
GET  /events/{project}
  → SSE stream of state change events

POST /webhooks
  Body: { "project": "pwork", "url": "http://localhost:19756/sep-events", "events": ["phase.changed", "blocker.added"] }
  → Register webhook

DELETE /webhooks/{id}
```

### System
```
GET  /health
  → SEP status + Ollama connectivity + DB stats

GET  /projects
  → List all tracked projects

POST /projects
  Body: { "name": "pwork", "description": "..." }

DELETE /memory/{project}/deep/before/{date}
  → Prune old deep memory
```

---

## How pwork Would Use SEP (Example — Not SEP's Concern)

This section exists only to validate the API design. SEP doesn't implement any of this.

```
1. Opus session finishes in pwork's PTY
2. pwork calls POST /ingest with the session log path
3. SEP processes → extracts decisions → updates state → emits events
4. pwork is subscribed to GET /events/pwork (SSE)
5. pwork receives { "event": "phase.changed", "domain": "ws", "new_phase": "ready" }
6. pwork decides to spawn a Sonnet session
7. pwork calls GET /memory/pwork/hot → gets 500-token context
8. pwork injects that context into the Sonnet session's CLAUDE.md
9. Sonnet executes with minimal, high-density context
```

---

## File Layout

```
sep/
├── Cargo.toml
├── DESIGN.md              ← this file
├── src/
│   ├── main.rs            ← Axum server, CLI args
│   ├── config.rs          ← Config (port, backend selection, db path, model path)
│   ├── ingester/
│   │   ├── mod.rs
│   │   ├── parser.rs      ← Log format parsers (JSONL, text, markdown)
│   │   └── filter.rs      ← Transcript cleaning/compression
│   ├── secretary/
│   │   ├── mod.rs         ← Secretary trait definition
│   │   ├── embedded.rs    ← llama-cpp-2 in-process backend (default)
│   │   ├── ollama.rs      ← Ollama HTTP backend
│   │   ├── openai.rs      ← OpenAI-compatible HTTP backend
│   │   ├── prompts.rs     ← Extraction prompt templates
│   │   └── extract.rs     ← Decision/delta/synthesis extraction logic
│   ├── memory/
│   │   ├── mod.rs
│   │   ├── hot.rs         ← Hot memory generation (<500 tok)
│   │   ├── state.rs       ← SQLite state memory (rusqlite)
│   │   ├── deep.rs        ← Deep memory archive (markdown files)
│   │   └── export.rs      ← project_brain.md generation
│   ├── events/
│   │   ├── mod.rs
│   │   ├── emitter.rs     ← Event bus (tokio::broadcast)
│   │   ├── sse.rs         ← SSE endpoint handler
│   │   └── webhook.rs     ← Webhook delivery
│   ├── api/
│   │   ├── mod.rs
│   │   ├── ingest.rs
│   │   ├── memory.rs
│   │   ├── events.rs
│   │   └── system.rs
│   └── db/
│       ├── mod.rs
│       └── migrations/
│           └── 001_initial.sql
├── prompts/
│   ├── extract_decisions.txt
│   ├── extract_state_delta.txt
│   ├── detect_phase.txt
│   └── synthesize_deep.txt
└── data/                  ← Runtime (per-project DBs and exports)
    └── {project}/
        ├── state.db
        ├── project_brain.md
        └── deep/
```

---

## What SEP Is

- A memory management daemon
- A structured data pipeline with a local LLM for extraction
- A state machine that tracks project truth over time
- An event source that tells consumers when state changes

## What SEP is NOT

- Not an orchestrator — it doesn't spawn sessions, pick models, or manage execution
- Not coupled to any specific AI tool — it processes text, not Claude-specific formats
- Not a UI — headless daemon, consumers provide their own interface
- Not a task manager — it tracks decisions and state, not work assignments

---

## v0.1 Scope (MVP)

1. Ingester: parse Claude Code JSONL logs into cleaned digests
2. Secretary trait + Embedded backend (llama-cpp-2 + GGUF)
3. Decision extraction with GBNF-constrained JSON output
4. SQLite state memory with upsert logic + decision history
5. Hot memory generation (<500 tokens)
6. `project_brain.md` export
7. Phase detection and `phase.changed` events via SSE
8. Core API: `/ingest`, `/memory/*/hot`, `/memory/*/state`, `/events/*`, `/health`

**Not in v0.1:** Ollama/OpenAI-compat backends (trivial to add), deep memory synthesis, webhook delivery, manual memory write endpoints, multi-format ingestion (text/markdown), pruning, fine-tuned model.

---

## Open Questions

1. **Chunking:** Large transcripts may exceed Qwen2.5's 32k context. Segment by conversation turns or tool-use boundaries?
2. **Confidence thresholds:** Secretary should only write high-confidence extractions. What's the right threshold — 0.7? 0.8? Configurable per project?
3. **Conflict resolution:** If a new ingestion contradicts an existing decision, should SEP auto-supersede or flag it as a conflict for human review?
4. **Embedding search:** For deep memory queries, add vector embeddings (Ollama supports embedding models)? Or full-text search sufficient for v1?
