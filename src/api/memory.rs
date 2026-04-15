use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::memory::{hot, state, export};
use crate::AppState;

#[derive(Deserialize, Default)]
pub struct StateQuery {
    pub source_session: Option<String>,
}

/// GET /memory/{project}/hot
pub async fn get_hot_memory(
    State(app): State<AppState>,
    Path(project): Path<String>,
) -> Result<String, (StatusCode, String)> {
    let conn = app.open_project_db(&project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e))
    })?;

    hot::generate_hot_memory(&conn, &project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Hot memory error: {}", e))
    })
}

/// GET /memory/{project}/state?source_session=...
pub async fn get_state(
    State(app): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<StateQuery>,
) -> Result<Json<StateResponse>, (StatusCode, String)> {
    let conn = app.open_project_db(&project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e))
    })?;

    let decisions = state::get_decisions(&conn, &project, query.source_session.as_deref()).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Query error: {}", e))
    })?;
    let blockers = state::get_active_blockers(&conn, &project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Query error: {}", e))
    })?;
    let questions = state::get_open_questions(&conn, &project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Query error: {}", e))
    })?;
    let phase = state::get_current_phase(&conn, &project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Query error: {}", e))
    })?;

    Ok(Json(StateResponse {
        project: project.clone(),
        decisions,
        blockers,
        open_questions: questions,
        current_phase: phase.map(|(domain, phase)| PhaseInfo { domain, phase }),
    }))
}

/// GET /memory/{project}/brain
pub async fn get_brain(
    State(app): State<AppState>,
    Path(project): Path<String>,
) -> Result<String, (StatusCode, String)> {
    let conn = app.open_project_db(&project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e))
    })?;

    export::export_brain(&conn, &project, &app.config.server.data_dir).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Export error: {}", e))
    })
}

#[derive(Serialize)]
pub struct StateResponse {
    pub project: String,
    pub decisions: Vec<state::StoredDecision>,
    pub blockers: Vec<String>,
    pub open_questions: Vec<String>,
    pub current_phase: Option<PhaseInfo>,
}

#[derive(Serialize)]
pub struct PhaseInfo {
    pub domain: String,
    pub phase: String,
}
