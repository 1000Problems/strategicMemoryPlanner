use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::ingester::mermaid::ExtractedDiagram;
use crate::secretary::extract::ExtractedDecision;

/// A decision stored in the state database.
#[derive(Debug, Clone, Serialize)]
pub struct StoredDecision {
    pub id: String,
    pub project: String,
    pub domain: String,
    pub decision: String,
    pub rationale: String,
    pub files: Vec<String>,
    pub source_session: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Ensure the project exists in the projects table.
pub fn ensure_project(conn: &Connection, project: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO projects (name) VALUES (?1)",
        params![project],
    )?;
    Ok(())
}

/// Upsert a decision. If a decision with the same domain already exists,
/// move the old version to history and update the current record.
pub fn upsert_decision(
    conn: &Connection,
    project: &str,
    decision: &ExtractedDecision,
    source_session: Option<&str>,
) -> Result<(String, bool)> {
    let id = uuid::Uuid::new_v4().to_string();
    let files_json = serde_json::to_string(&decision.files)?;
    let alts_json = serde_json::to_string(&decision.alternatives_rejected)?;

    // Check for existing decision in the same domain with similar content
    let existing: Option<(String, String, String)> = conn
        .prepare(
            "SELECT id, decision, rationale FROM decisions
             WHERE project = ?1 AND domain = ?2
             ORDER BY updated_at DESC LIMIT 1"
        )?
        .query_row(params![project, decision.domain], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .ok();

    if let Some((existing_id, old_decision, old_rationale)) = existing {
        // Check if it's actually different
        if old_decision == decision.decision {
            return Ok((existing_id, false)); // No change
        }

        // Move old version to history
        conn.execute(
            "INSERT INTO decision_history (id, decision_id, old_decision, old_rationale)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                uuid::Uuid::new_v4().to_string(),
                existing_id,
                old_decision,
                old_rationale,
            ],
        )?;

        // Update the existing record
        conn.execute(
            "UPDATE decisions SET decision = ?1, rationale = ?2, files = ?3,
             alternatives_rejected = ?4, source_session = ?5, updated_at = datetime('now')
             WHERE id = ?6",
            params![
                decision.decision,
                decision.rationale,
                files_json,
                alts_json,
                source_session,
                existing_id,
            ],
        )?;

        tracing::info!(
            domain = decision.domain,
            "Decision updated (old version archived)"
        );

        Ok((existing_id, true)) // Updated
    } else {
        // Insert new decision
        conn.execute(
            "INSERT INTO decisions (id, project, domain, decision, rationale, files, alternatives_rejected, source_session)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                project,
                decision.domain,
                decision.decision,
                decision.rationale,
                files_json,
                alts_json,
                source_session,
            ],
        )?;

        tracing::info!(domain = decision.domain, "New decision stored");

        Ok((id, true)) // New
    }
}

/// Get decisions for a project, optionally filtered by source session.
pub fn get_decisions(conn: &Connection, project: &str, source_session: Option<&str>) -> Result<Vec<StoredDecision>> {
    let map_row = |row: &rusqlite::Row| {
        let files_str: String = row.get(5)?;
        Ok(StoredDecision {
            id: row.get(0)?,
            project: row.get(1)?,
            domain: row.get(2)?,
            decision: row.get(3)?,
            rationale: row.get(4)?,
            files: serde_json::from_str(&files_str).unwrap_or_default(),
            source_session: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    };

    if let Some(sess) = source_session {
        let mut stmt = conn.prepare(
            "SELECT id, project, domain, decision, rationale, files, source_session, created_at, updated_at
             FROM decisions WHERE project = ?1 AND source_session = ?2
             ORDER BY domain, updated_at DESC"
        )?;
        let rows: Vec<StoredDecision> = stmt.query_map(params![project, sess], map_row)?.filter_map(|r| r.ok()).collect();
        Ok(rows)
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, project, domain, decision, rationale, files, source_session, created_at, updated_at
             FROM decisions WHERE project = ?1 ORDER BY domain, updated_at DESC"
        )?;
        let rows: Vec<StoredDecision> = stmt.query_map(params![project], map_row)?.filter_map(|r| r.ok()).collect();
        Ok(rows)
    }
}

/// Get active blockers for a project.
pub fn get_active_blockers(conn: &Connection, project: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT description FROM blockers WHERE project = ?1 AND status = 'active'"
    )?;
    let rows = stmt.query_map(params![project], |row| row.get(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Get open questions for a project.
pub fn get_open_questions(conn: &Connection, project: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT question FROM open_questions WHERE project = ?1 AND status = 'open'"
    )?;
    let rows = stmt.query_map(params![project], |row| row.get(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Get the current phase for a project (latest entry).
pub fn get_current_phase(conn: &Connection, project: &str) -> Result<Option<(String, String)>> {
    conn.prepare(
        "SELECT domain, phase FROM phase_log
         WHERE project = ?1 AND ended_at IS NULL
         ORDER BY started_at DESC LIMIT 1"
    )?
    .query_row(params![project], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })
    .ok()
    .map(|r| Ok(r))
    .transpose()
}

/// Update the phase — close the old one and open a new entry.
pub fn update_phase(
    conn: &Connection,
    project: &str,
    domain: &str,
    new_phase: &str,
) -> Result<bool> {
    // Close any existing open phase for this domain
    conn.execute(
        "UPDATE phase_log SET ended_at = datetime('now')
         WHERE project = ?1 AND domain = ?2 AND ended_at IS NULL",
        params![project, domain],
    )?;

    // Insert new phase
    conn.execute(
        "INSERT INTO phase_log (id, project, phase, domain)
         VALUES (?1, ?2, ?3, ?4)",
        params![uuid::Uuid::new_v4().to_string(), project, new_phase, domain],
    )?;

    Ok(true)
}

/// Log an ingestion job.
pub fn log_ingestion(
    conn: &Connection,
    id: &str,
    project: &str,
    source_path: &str,
    status: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO ingestion_log (id, project, source_path, status)
         VALUES (?1, ?2, ?3, ?4)",
        params![id, project, source_path, status],
    )?;
    Ok(())
}

// ─── Mermaid Diagrams ─────────────────────────────────────────────────────────

/// A mermaid diagram stored in the state database.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredDiagram {
    pub id: String,
    pub project: String,
    pub title: Option<String>,
    pub diagram_type: String,
    pub content: String,
    pub fingerprint: String,
    pub source_session: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Insert a diagram if its fingerprint doesn't exist yet.
/// Returns true if new, false if already stored (idempotent re-ingestion).
pub fn upsert_mermaid(
    conn: &Connection,
    project: &str,
    diagram: &ExtractedDiagram,
    source_session: Option<&str>,
) -> Result<bool> {
    let id = uuid::Uuid::new_v4().to_string();
    let rows = conn.execute(
        "INSERT OR IGNORE INTO mermaid_diagrams
         (id, project, title, diagram_type, content, fingerprint, source_session)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            project,
            diagram.title,
            diagram.diagram_type,
            diagram.content,
            diagram.fingerprint,
            source_session,
        ],
    )?;
    Ok(rows > 0)
}

/// Get mermaid diagrams for a project, optionally filtered by source session.
pub fn get_mermaid_diagrams(conn: &Connection, project: &str, source_session: Option<&str>) -> Result<Vec<StoredDiagram>> {
    let map_row = |row: &rusqlite::Row| {
        Ok(StoredDiagram {
            id: row.get(0)?,
            project: row.get(1)?,
            title: row.get(2)?,
            diagram_type: row.get(3)?,
            content: row.get(4)?,
            fingerprint: row.get(5)?,
            source_session: row.get(6)?,
            version: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        })
    };

    if let Some(sess) = source_session {
        let mut stmt = conn.prepare(
            "SELECT id, project, title, diagram_type, content, fingerprint,
                    source_session, version, created_at, updated_at
             FROM mermaid_diagrams WHERE project = ?1 AND source_session = ?2
             ORDER BY created_at DESC",
        )?;
        let rows: Vec<StoredDiagram> = stmt.query_map(params![project, sess], map_row)?.filter_map(|r| r.ok()).collect();
        Ok(rows)
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, project, title, diagram_type, content, fingerprint,
                    source_session, version, created_at, updated_at
             FROM mermaid_diagrams WHERE project = ?1
             ORDER BY created_at DESC",
        )?;
        let rows: Vec<StoredDiagram> = stmt.query_map(params![project], map_row)?.filter_map(|r| r.ok()).collect();
        Ok(rows)
    }
}

/// Delete a mermaid diagram by id.
pub fn delete_mermaid(conn: &Connection, project: &str, id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM mermaid_diagrams WHERE id = ?1 AND project = ?2",
        params![id, project],
    )?;
    Ok(())
}

/// Update ingestion job status.
pub fn update_ingestion(
    conn: &Connection,
    id: &str,
    status: &str,
    raw_tokens: Option<usize>,
    digest_tokens: Option<usize>,
    error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE ingestion_log SET status = ?1, raw_tokens = ?2, digest_tokens = ?3,
         error = ?4, completed_at = datetime('now')
         WHERE id = ?5",
        params![
            status,
            raw_tokens.map(|t| t as i64),
            digest_tokens.map(|t| t as i64),
            error,
            id,
        ],
    )?;
    Ok(())
}
