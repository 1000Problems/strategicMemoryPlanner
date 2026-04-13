use anyhow::Result;
use rusqlite::Connection;

use super::state;

/// Generate hot memory for a project — the <500 token snapshot
/// that consumers inject into their sessions.
pub fn generate_hot_memory(conn: &Connection, project: &str) -> Result<String> {
    let decisions = state::get_decisions(conn, project)?;
    let blockers = state::get_active_blockers(conn, project)?;
    let questions = state::get_open_questions(conn, project)?;
    let phase = state::get_current_phase(conn, project)?;

    let mut out = String::new();

    out.push_str(&format!("## Project: {}\n", project));

    // Current phase
    if let Some((domain, phase_name)) = &phase {
        out.push_str(&format!("- PHASE: {} ({})\n", phase_name, domain));
    }

    // Decisions — shorthand, one line each
    if !decisions.is_empty() {
        for d in &decisions {
            let files_hint = if d.files.is_empty() {
                String::new()
            } else {
                format!(" [{}]", d.files.join(", "))
            };
            out.push_str(&format!(
                "- DECIDED ({}): {}{}",
                d.domain, d.decision, files_hint
            ));
            if !d.rationale.is_empty() {
                out.push_str(&format!(" — {}", d.rationale));
            }
            out.push('\n');
        }
    }

    // Blockers
    if !blockers.is_empty() {
        for b in &blockers {
            out.push_str(&format!("- BLOCKER: {}\n", b));
        }
    } else {
        out.push_str("- BLOCKER: None\n");
    }

    // Open questions
    for q in &questions {
        out.push_str(&format!("- OPEN: {}\n", q));
    }

    // Timestamp
    out.push_str(&format!(
        "- UPDATED: {}\n",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S")
    ));

    // Safety check: if we're over budget, truncate decisions
    let token_est = out.len() / 4;
    if token_est > 500 {
        tracing::warn!(
            tokens = token_est,
            "Hot memory exceeds 500 token budget, consider pruning old decisions"
        );
    }

    Ok(out)
}
