use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

use super::state;

/// Generate and write the project_brain.md file.
/// This is the human-readable view of the state memory.
pub fn export_brain(conn: &Connection, project: &str, data_dir: &Path) -> Result<String> {
    let decisions = state::get_decisions(conn, project)?;
    let blockers = state::get_active_blockers(conn, project)?;
    let questions = state::get_open_questions(conn, project)?;
    let phase = state::get_current_phase(conn, project)?;

    let mut md = String::new();

    md.push_str(&format!("# Project Brain — {}\n", project));
    md.push_str(&format!(
        "Generated: {}\n\n",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S")
    ));

    // Phase
    if let Some((domain, phase_name)) = &phase {
        md.push_str("## Current Phase\n");
        md.push_str(&format!("- {}: {}\n\n", domain, phase_name));
    }

    // Decisions by domain
    if !decisions.is_empty() {
        md.push_str("## Active Decisions\n");
        let mut current_domain = String::new();
        for d in &decisions {
            if d.domain != current_domain {
                current_domain = d.domain.clone();
                md.push_str(&format!("\n### {}\n", current_domain));
            }
            md.push_str(&format!("- **{}**", d.decision));
            if !d.rationale.is_empty() {
                md.push_str(&format!(" — {}", d.rationale));
            }
            md.push('\n');
            if !d.files.is_empty() {
                md.push_str(&format!("  Files: {}\n", d.files.join(", ")));
            }
        }
        md.push('\n');
    }

    // Blockers
    md.push_str("## Blockers\n");
    if blockers.is_empty() {
        md.push_str("- None\n");
    } else {
        for b in &blockers {
            md.push_str(&format!("- {}\n", b));
        }
    }
    md.push('\n');

    // Open Questions
    if !questions.is_empty() {
        md.push_str("## Open Questions\n");
        for q in &questions {
            md.push_str(&format!("- {}\n", q));
        }
        md.push('\n');
    }

    // Write to disk
    let project_dir = data_dir.join(project);
    std::fs::create_dir_all(&project_dir)?;
    let brain_path = project_dir.join("project_brain.md");
    std::fs::write(&brain_path, &md)
        .with_context(|| format!("Failed to write {}", brain_path.display()))?;

    tracing::info!(path = %brain_path.display(), "Exported project_brain.md");

    Ok(md)
}
