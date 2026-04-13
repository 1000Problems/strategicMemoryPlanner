use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::events::Event;
use crate::ingester;
use crate::memory::{export, state};
use crate::secretary::extract::{detect_phase, extract_decisions};
use crate::AppState;

#[derive(Deserialize)]
pub struct IngestRequest {
    pub project: String,
    pub source: String, // Path to the log file
    #[serde(default = "default_format")]
    pub format: String, // "auto", "jsonl", "json", "text"
}

fn default_format() -> String { "auto".to_string() }

#[derive(Serialize)]
pub struct IngestResponse {
    pub job_id: String,
    pub status: String,
}

#[derive(Serialize)]
pub struct IngestResult {
    pub job_id: String,
    pub status: String,
    pub raw_tokens: usize,
    pub digest_tokens: usize,
    pub compression_ratio: f64,
    pub decisions_extracted: usize,
    pub phase_detected: Option<String>,
}

/// POST /ingest — ingest a session log and extract meaning.
/// For MVP this runs synchronously. TODO: make async with job tracking.
pub async fn ingest_log(
    State(app): State<AppState>,
    Json(req): Json<IngestRequest>,
) -> Result<Json<IngestResult>, (StatusCode, String)> {
    let job_id = uuid::Uuid::new_v4().to_string();
    let source_path = PathBuf::from(&req.source);

    tracing::info!(
        job_id = %job_id,
        project = %req.project,
        source = %req.source,
        "Starting ingestion"
    );

    // Open DB and ensure project exists
    let conn = app.open_project_db(&req.project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e))
    })?;
    state::ensure_project(&conn, &req.project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Project error: {}", e))
    })?;

    // Log the ingestion
    state::log_ingestion(&conn, &job_id, &req.project, &req.source, "processing")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Log error: {}", e)))?;

    // Phase 1: Ingest — parse and filter the log file
    let digest = ingester::ingest(&source_path, &app.config.ingester).map_err(|e| {
        let _ = state::update_ingestion(&conn, &job_id, "failed", None, None, Some(&e.to_string()));
        (StatusCode::BAD_REQUEST, format!("Ingestion failed: {}", e))
    })?;

    tracing::info!(
        raw = digest.raw_token_estimate,
        digest = digest.token_estimate,
        compression = format!("{:.1}x", digest.compression_ratio),
        "Ingestion complete, starting extraction"
    );

    // Phase 2: Extract decisions via Secretary
    let decisions = extract_decisions(
        app.secretary.as_ref(),
        &app.prompts,
        &digest,
    )
    .await
    .map_err(|e| {
        let _ = state::update_ingestion(&conn, &job_id, "failed", None, None, Some(&e.to_string()));
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Extraction failed: {}", e))
    })?;

    // Store decisions
    let mut new_count = 0;
    for decision in &decisions {
        match state::upsert_decision(&conn, &req.project, decision) {
            Ok((_id, is_new)) => {
                if is_new {
                    new_count += 1;
                    app.events.emit(Event::DecisionNew {
                        project: req.project.clone(),
                        domain: decision.domain.clone(),
                        decision: decision.decision.clone(),
                    });
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to store decision, skipping");
            }
        }
    }

    // Phase 3: Detect phase
    let phase_detected = detect_phase(
        app.secretary.as_ref(),
        &app.prompts,
        &digest,
    )
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, "Phase detection failed (non-fatal)");
    })
    .ok()
    .flatten();

    let phase_str = phase_detected.as_ref().map(|p| {
        let old_phase = state::get_current_phase(&conn, &req.project)
            .ok()
            .flatten()
            .map(|(_, p)| p)
            .unwrap_or_else(|| "unknown".to_string());

        if old_phase != p.phase {
            let _ = state::update_phase(&conn, &req.project, &p.domain, &p.phase);
            app.events.emit(Event::PhaseChanged {
                project: req.project.clone(),
                domain: p.domain.clone(),
                old_phase: old_phase.clone(),
                new_phase: p.phase.clone(),
            });
        }
        p.phase.clone()
    });

    // Export brain and hot memory
    let _ = export::export_brain(&conn, &req.project, &app.config.server.data_dir);

    // Update ingestion log
    let _ = state::update_ingestion(
        &conn,
        &job_id,
        "complete",
        Some(digest.raw_token_estimate),
        Some(digest.token_estimate),
        None,
    );

    // Emit completion event
    app.events.emit(Event::IngestionComplete {
        project: req.project.clone(),
        job_id: job_id.clone(),
        raw_tokens: digest.raw_token_estimate,
        digest_tokens: digest.token_estimate,
        decisions_extracted: decisions.len(),
    });

    tracing::info!(
        job_id = %job_id,
        decisions = decisions.len(),
        new = new_count,
        phase = ?phase_str,
        "Ingestion pipeline complete"
    );

    Ok(Json(IngestResult {
        job_id,
        status: "complete".to_string(),
        raw_tokens: digest.raw_token_estimate,
        digest_tokens: digest.token_estimate,
        compression_ratio: digest.compression_ratio,
        decisions_extracted: decisions.len(),
        phase_detected: phase_str,
    }))
}
