-- SEP v0.1 schema

CREATE TABLE IF NOT EXISTS projects (
    name TEXT PRIMARY KEY,
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS decisions (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL REFERENCES projects(name),
    domain TEXT,
    decision TEXT NOT NULL,
    rationale TEXT,
    alternatives_rejected TEXT,  -- JSON array
    files TEXT,                  -- JSON array
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_decisions_project ON decisions(project);
CREATE INDEX IF NOT EXISTS idx_decisions_domain ON decisions(project, domain);

CREATE TABLE IF NOT EXISTS decision_history (
    id TEXT PRIMARY KEY,
    decision_id TEXT NOT NULL,
    old_decision TEXT,
    old_rationale TEXT,
    superseded_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS blockers (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL REFERENCES projects(name),
    description TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',  -- active | resolved
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    resolved_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_blockers_project ON blockers(project, status);

CREATE TABLE IF NOT EXISTS open_questions (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL REFERENCES projects(name),
    question TEXT NOT NULL,
    context TEXT,
    status TEXT NOT NULL DEFAULT 'open',  -- open | answered
    answer TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    answered_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_questions_project ON open_questions(project, status);

CREATE TABLE IF NOT EXISTS phase_log (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL REFERENCES projects(name),
    phase TEXT NOT NULL,  -- exploring | design | ready | blocked | review | done
    domain TEXT,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    ended_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_phase_project ON phase_log(project);

CREATE TABLE IF NOT EXISTS ingestion_log (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    source_path TEXT,
    raw_tokens INTEGER,
    digest_tokens INTEGER,
    extractions_run TEXT,  -- JSON array of modes
    status TEXT NOT NULL DEFAULT 'pending',  -- pending | processing | complete | failed
    error TEXT,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    completed_at TEXT
);
