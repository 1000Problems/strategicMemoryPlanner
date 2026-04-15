use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::memory::state::{self, StoredDiagram};
use crate::AppState;

#[derive(Deserialize, Default)]
pub struct DiagramQuery {
    pub source_session: Option<String>,
}

/// GET /mermaid/{project}?source_session=... — list diagrams for a project.
pub async fn list_diagrams(
    State(app): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<DiagramQuery>,
) -> Result<Json<Vec<StoredDiagram>>, (StatusCode, String)> {
    let conn = app.open_project_db(&project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e))
    })?;

    let diagrams = state::get_mermaid_diagrams(&conn, &project, query.source_session.as_deref()).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Query error: {}", e))
    })?;

    Ok(Json(diagrams))
}

/// DELETE /mermaid/{project}/{id} — delete a diagram.
pub async fn delete_diagram(
    State(app): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let conn = app.open_project_db(&project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e))
    })?;

    state::delete_mermaid(&conn, &project, &id).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Delete error: {}", e))
    })?;

    // Also remove .mmd file if it exists (best effort)
    let mmd_dir = app.config.server.data_dir.join(&project).join("mermaid");
    // We don't store the filename directly, so we'd need the fingerprint.
    // For now, skip filesystem cleanup — the DB record is the source of truth.
    let _ = mmd_dir;

    Ok(StatusCode::NO_CONTENT)
}
