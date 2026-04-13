use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::AppState;

#[derive(Deserialize)]
pub struct DebugQueryParams {
    pub project: String,
    pub sql: String,
}

#[derive(Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<serde_json::Value>,
}

/// GET /debug/query?project=xxx&sql=SELECT...
/// Read-only SQL explorer. Only SELECT statements are allowed.
/// Gated behind config.server.debug = true.
pub async fn debug_query(
    State(app): State<AppState>,
    Query(params): Query<DebugQueryParams>,
) -> Result<Json<QueryResult>, (StatusCode, String)> {
    if !app.config.server.debug {
        return Err((StatusCode::NOT_FOUND, "Debug endpoints are disabled".to_string()));
    }

    // Only allow SELECT — reject anything else to prevent mutation
    let sql_trimmed = params.sql.trim().to_lowercase();
    if !sql_trimmed.starts_with("select") {
        return Err((
            StatusCode::BAD_REQUEST,
            "Only SELECT statements are permitted".to_string(),
        ));
    }

    let conn = app.open_project_db(&params.project).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e))
    })?;

    let mut stmt = conn.prepare(&params.sql).map_err(|e| {
        (StatusCode::BAD_REQUEST, format!("SQL error: {}", e))
    })?;

    let columns: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();

    let col_count = columns.len();

    let rows: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            let mut map = serde_json::Map::new();
            for i in 0..col_count {
                let col_name = columns[i].clone();
                // Try each supported type in order
                let val = if let Ok(v) = row.get::<_, Option<i64>>(i) {
                    match v {
                        Some(n) => serde_json::Value::Number(n.into()),
                        None => serde_json::Value::Null,
                    }
                } else if let Ok(v) = row.get::<_, Option<f64>>(i) {
                    match v {
                        Some(f) => serde_json::json!(f),
                        None => serde_json::Value::Null,
                    }
                } else if let Ok(v) = row.get::<_, Option<String>>(i) {
                    match v {
                        Some(s) => serde_json::Value::String(s),
                        None => serde_json::Value::Null,
                    }
                } else {
                    serde_json::Value::Null
                };
                map.insert(col_name, val);
            }
            Ok(serde_json::Value::Object(map))
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Query error: {}", e)))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Json(QueryResult { columns, rows }))
}
