CREATE TABLE IF NOT EXISTS mermaid_diagrams (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL REFERENCES projects(name),
    title TEXT,
    diagram_type TEXT NOT NULL DEFAULT 'unknown',
    content TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    source_session TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_mermaid_fingerprint ON mermaid_diagrams(project, fingerprint);
CREATE INDEX IF NOT EXISTS idx_mermaid_project ON mermaid_diagrams(project);
