use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::memory::state;
use crate::AppState;

#[derive(Serialize)]
pub struct ProjectSummary {
    pub name: String,
    pub path: Option<String>,
    pub current_phase: Option<String>,
    pub decision_count: usize,
    pub last_ingestion: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    pub path: Option<String>,
}

/// GET /projects — list all projects discovered in the data directory.
pub async fn list_projects(
    State(app): State<AppState>,
) -> Result<Json<Vec<ProjectSummary>>, (StatusCode, String)> {
    let data_dir = &app.config.server.data_dir;

    let entries = std::fs::read_dir(data_dir).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to read data dir: {}", e))
    })?;

    let mut projects = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let db_path = path.join("state.db");
        if !db_path.exists() {
            continue;
        }

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if name.is_empty() {
            continue;
        }

        let summary = match app.open_project_db(&name) {
            Ok(conn) => {
                let current_phase = state::get_current_phase(&conn, &name)
                    .ok()
                    .flatten()
                    .map(|(_, phase)| phase);

                let decision_count = conn
                    .query_row(
                        "SELECT COUNT(*) FROM decisions WHERE project = ?1",
                        rusqlite::params![name],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap_or(0) as usize;

                let last_ingestion = conn
                    .query_row(
                        "SELECT completed_at FROM ingestion_log WHERE project = ?1
                         AND status = 'complete' ORDER BY completed_at DESC LIMIT 1",
                        rusqlite::params![name],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .ok()
                    .flatten();

                let path: Option<String> = conn
                    .query_row(
                        "SELECT path FROM projects WHERE name = ?1",
                        rusqlite::params![name],
                        |row| row.get(0),
                    )
                    .ok();

                ProjectSummary {
                    name,
                    path,
                    current_phase,
                    decision_count,
                    last_ingestion,
                }
            }
            Err(_) => ProjectSummary {
                name,
                path: None,
                current_phase: None,
                decision_count: 0,
                last_ingestion: None,
            },
        };

        projects.push(summary);
    }

    projects.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(projects))
}

/// POST /projects — create a new project.
pub async fn create_project(
    State(app): State<AppState>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Json<ProjectSummary>, (StatusCode, String)> {
    if req.name.is_empty() || req.name.contains('/') || req.name.contains("..") {
        return Err((StatusCode::BAD_REQUEST, "Invalid project name".to_string()));
    }

    // Validate path exists and has CLAUDE.md
    if let Some(ref path) = req.path {
        let dir = std::path::Path::new(path);
        if !dir.exists() {
            return Err((StatusCode::BAD_REQUEST, format!("Directory does not exist: {path}")));
        }
        if !dir.join("CLAUDE.md").exists() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("No CLAUDE.md found in {path}. Projects must have a CLAUDE.md file."),
            ));
        }
    }

    let conn = app.open_project_db(&req.name).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e))
    })?;

    state::ensure_project(&conn, &req.name).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to create project: {}", e))
    })?;

    // Store path if provided
    if let Some(ref path) = req.path {
        conn.execute(
            "UPDATE projects SET path = ?1 WHERE name = ?2",
            rusqlite::params![path, req.name],
        ).map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to set path: {}", e))
        })?;
    }

    tracing::info!(project = %req.name, path = ?req.path, "Project created");

    Ok(Json(ProjectSummary {
        name: req.name,
        path: req.path,
        current_phase: None,
        decision_count: 0,
        last_ingestion: None,
    }))
}

/// DELETE /projects/{name} — remove a project and its data directory.
pub async fn delete_project(
    State(app): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let project_dir = app.config.project_data_dir(&name);

    if project_dir.exists() {
        std::fs::remove_dir_all(&project_dir).map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to delete project data: {e}"))
        })?;
    }

    tracing::info!(project = %name, "Project deleted");
    Ok(StatusCode::NO_CONTENT)
}
