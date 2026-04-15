use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Datelike;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::time::UNIX_EPOCH;

use crate::AppState;

// ─── Response Types ───────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct SessionsListEntry {
    pub slug: String,
    pub session_count: usize,
    pub latest_modified: Option<u64>, // unix timestamp seconds
}

#[derive(Serialize)]
pub struct SessionEntry {
    pub id: String,
    pub title: String,
    pub model: Option<String>,
    pub size_bytes: u64,
    pub line_count: usize,
    pub modified_at: Option<u64>,
    pub active: bool,
    pub user_turns: usize,
    pub output_tokens: u64,
    pub context_size: u64,
}

#[derive(Serialize)]
pub struct SessionStats {
    pub session_id: String,
    pub title: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub models_used: Vec<String>,
    pub active: bool,
    pub user_turns: usize,
    pub assistant_turns: usize,
    pub total_output_tokens: u64,
    pub context_size: u64,
    pub tools: std::collections::HashMap<String, usize>,
    pub files_read: Vec<String>,
    pub files_edited: Vec<String>,
    pub started_at: Option<u64>,
    pub last_activity: Option<u64>,
    pub suggestions: Vec<String>,
}

#[derive(Serialize)]
pub struct BillingResponse {
    pub today: BillingPeriod,
    pub billing_cycle: BillingCycle,
    pub daily: Vec<DailyUsage>,
}

#[derive(Serialize)]
pub struct BillingPeriod {
    pub output_tokens: u64,
    pub sessions: usize,
}

#[derive(Serialize)]
pub struct BillingCycle {
    pub start: String,
    pub output_tokens: u64,
    pub sessions: usize,
}

#[derive(Serialize)]
pub struct DailyUsage {
    pub date: String,
    pub output_tokens: u64,
    pub sessions: usize,
}

#[derive(Serialize)]
pub struct ParsedSession {
    pub session_id: String,
    pub source_path: String,
    pub messages: Vec<ParsedMessage>,
}

#[derive(Serialize)]
pub struct ParsedMessage {
    pub role: String, // "user" | "assistant" | "system"
    pub content_blocks: Vec<ContentBlock>,
    pub usage: Option<Usage>,
}

#[derive(Serialize)]
pub struct ContentBlock {
    pub kind: String, // "text" | "thinking" | "tool_use" | "tool_result"
    pub text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    pub tool_input: Option<Value>,
}

#[derive(Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

// ─── GET /sessions ────────────────────────────────────────────────────────────

/// List all Claude Code project slugs found in sessions_dir.
pub async fn list_session_projects(
    State(app): State<AppState>,
) -> Result<Json<Vec<SessionsListEntry>>, (StatusCode, String)> {
    let sessions_dir = &app.config.server.sessions_dir;

    let entries = fs::read_dir(sessions_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Cannot read sessions dir {}: {}", sessions_dir.display(), e),
        )
    })?;

    let mut result: Vec<SessionsListEntry> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let slug = path.file_name().unwrap_or_default().to_string_lossy().to_string();

        // Count .jsonl files
        let jsonl_files: Vec<_> = fs::read_dir(&path)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        e.path().extension().and_then(|x| x.to_str()) == Some("jsonl")
                    })
                    .collect()
            })
            .unwrap_or_default();

        let session_count = jsonl_files.len();

        // Find latest modified among jsonl files
        let latest_modified = jsonl_files
            .iter()
            .filter_map(|e| {
                e.metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
            })
            .max();

        result.push(SessionsListEntry {
            slug,
            session_count,
            latest_modified,
        });
    }

    // Sort by latest_modified descending
    result.sort_by(|a, b| b.latest_modified.cmp(&a.latest_modified));

    Ok(Json(result))
}

// ─── GET /sessions/:slug ──────────────────────────────────────────────────────

/// List all .jsonl session files for a given project slug.
pub async fn list_sessions(
    State(app): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<Vec<SessionEntry>>, (StatusCode, String)> {
    let project_dir = app.config.server.sessions_dir.join(&slug);

    if !project_dir.exists() {
        return Err((StatusCode::NOT_FOUND, format!("No sessions for slug: {slug}")));
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut sessions: Vec<SessionEntry> = fs::read_dir(&project_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .filter_map(|e| {
            let path = e.path();
            let id = path.file_stem()?.to_string_lossy().to_string();
            let meta = e.metadata().ok()?;
            let size_bytes = meta.len();
            let modified_at = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs());

            let active = modified_at.map(|t| now_secs - t < 300).unwrap_or(false);

            // Quick scan for title, model, tokens, context
            let summary = quick_scan_session(&path);

            Some(SessionEntry {
                id,
                title: summary.title,
                model: summary.model,
                size_bytes,
                line_count: summary.line_count,
                modified_at,
                active,
                user_turns: summary.user_turns,
                output_tokens: summary.output_tokens,
                context_size: summary.context_size,
            })
        })
        .collect();

    sessions.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));

    Ok(Json(sessions))
}

// ─── GET /sessions/:slug/:id ──────────────────────────────────────────────────

/// Parse and return messages from a single .jsonl session file.
pub async fn get_session(
    State(app): State<AppState>,
    Path((slug, id)): Path<(String, String)>,
) -> Result<Json<ParsedSession>, (StatusCode, String)> {
    let file_path = app
        .config
        .server
        .sessions_dir
        .join(&slug)
        .join(format!("{id}.jsonl"));

    if !file_path.exists() {
        return Err((StatusCode::NOT_FOUND, format!("Session not found: {id}")));
    }

    let file = fs::File::open(&file_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut messages: Vec<ParsedMessage> = Vec::new();

    for line in BufReader::new(file).lines().flatten() {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let Ok(entry): Result<Value, _> = serde_json::from_str(&line) else {
            continue;
        };

        let entry_type = entry["type"].as_str().unwrap_or("");

        match entry_type {
            "user" => {
                if let Some(msg) = parse_user_message(&entry) {
                    messages.push(msg);
                }
            }
            "assistant" => {
                if let Some(msg) = parse_assistant_message(&entry) {
                    messages.push(msg);
                }
            }
            _ => {} // skip system, file-history-snapshot, attachment, etc.
        }
    }

    Ok(Json(ParsedSession {
        session_id: id,
        source_path: file_path.to_string_lossy().to_string(),
        messages,
    }))
}

// ─── Parsers ──────────────────────────────────────────────────────────────────

fn parse_user_message(entry: &Value) -> Option<ParsedMessage> {
    let msg = &entry["message"];
    let content = &msg["content"];

    let blocks = match content {
        Value::String(s) => vec![ContentBlock {
            kind: "text".to_string(),
            text: Some(s.clone()),
            tool_name: None,
            tool_use_id: None,
            tool_input: None,
        }],
        Value::Array(arr) => arr.iter().filter_map(parse_content_block).collect(),
        _ => return None,
    };

    if blocks.is_empty() {
        return None;
    }

    Some(ParsedMessage {
        role: "user".to_string(),
        content_blocks: blocks,
        usage: None,
    })
}

fn parse_assistant_message(entry: &Value) -> Option<ParsedMessage> {
    let msg = &entry["message"];
    let content = msg["content"].as_array()?;

    let blocks: Vec<ContentBlock> = content.iter().filter_map(parse_content_block).collect();

    if blocks.is_empty() {
        return None;
    }

    let usage = parse_usage(&msg["usage"]);

    Some(ParsedMessage {
        role: "assistant".to_string(),
        content_blocks: blocks,
        usage,
    })
}

fn parse_content_block(block: &Value) -> Option<ContentBlock> {
    let kind = block["type"].as_str()?;

    match kind {
        "text" => Some(ContentBlock {
            kind: "text".to_string(),
            text: block["text"].as_str().map(|s| s.to_string()),
            tool_name: None,
            tool_use_id: None,
            tool_input: None,
        }),
        "thinking" => Some(ContentBlock {
            kind: "thinking".to_string(),
            text: block["thinking"].as_str().map(|s| s.to_string()),
            tool_name: None,
            tool_use_id: None,
            tool_input: None,
        }),
        "tool_use" => Some(ContentBlock {
            kind: "tool_use".to_string(),
            text: None,
            tool_name: block["name"].as_str().map(|s| s.to_string()),
            tool_use_id: block["id"].as_str().map(|s| s.to_string()),
            tool_input: Some(block["input"].clone()),
        }),
        "tool_result" => {
            // content can be string or array
            let text = match &block["content"] {
                Value::String(s) => Some(s.clone()),
                Value::Array(arr) => arr
                    .iter()
                    .find(|b| b["type"] == "text")
                    .and_then(|b| b["text"].as_str())
                    .map(|s| s.to_string()),
                _ => None,
            };
            Some(ContentBlock {
                kind: "tool_result".to_string(),
                text,
                tool_name: None,
                tool_use_id: block["tool_use_id"].as_str().map(|s| s.to_string()),
                tool_input: None,
            })
        }
        _ => None,
    }
}

fn parse_usage(usage: &Value) -> Option<Usage> {
    if usage.is_null() || !usage.is_object() {
        return None;
    }
    Some(Usage {
        input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
        cache_read_input_tokens: usage["cache_read_input_tokens"].as_u64(),
        cache_creation_input_tokens: usage["cache_creation_input_tokens"].as_u64(),
    })
}

// ─── Quick Scan ───────────────────────────────────────────────────────────────

struct QuickSummary {
    title: String,
    model: Option<String>,
    line_count: usize,
    user_turns: usize,
    output_tokens: u64,
    context_size: u64,
}

fn quick_scan_session(path: &std::path::Path) -> QuickSummary {
    let mut title = String::new();
    let mut model: Option<String> = None;
    let mut line_count = 0;
    let mut user_turns = 0;
    let mut output_tokens: u64 = 0;
    let mut last_context_size: u64 = 0;

    let Ok(file) = fs::File::open(path) else {
        return QuickSummary { title: "(unreadable)".into(), model: None, line_count: 0, user_turns: 0, output_tokens: 0, context_size: 0 };
    };

    for line in BufReader::new(file).lines().flatten() {
        line_count += 1;
        let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
        let entry_type = entry["type"].as_str().unwrap_or("");

        match entry_type {
            "user" => {
                let msg = &entry["message"];
                let content = &msg["content"];
                // Count user turns (only string content = real user input)
                if let Value::String(s) = content {
                    user_turns += 1;
                    // First meaningful user message as title
                    if title.is_empty() {
                        let trimmed = s.trim();
                        if !trimmed.is_empty()
                            && !trimmed.starts_with("<command")
                            && !trimmed.starts_with("/init")
                        {
                            title = trimmed.chars().take(80).collect();
                        }
                    }
                }
            }
            "assistant" => {
                let msg = &entry["message"];
                let usage = &msg["usage"];
                if let Some(out) = usage["output_tokens"].as_u64() {
                    output_tokens += out;
                }
                let input = usage["input_tokens"].as_u64().unwrap_or(0);
                let cache = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                if input + cache > 0 {
                    last_context_size = input + cache;
                }
                if model.is_none() {
                    model = msg["model"].as_str().map(|s| s.to_string());
                } else if let Some(m) = msg["model"].as_str() {
                    // Track latest model
                    model = Some(m.to_string());
                }
            }
            _ => {}
        }
    }

    if title.is_empty() {
        title = "(no title)".into();
    }

    QuickSummary { title, model, line_count, user_turns, output_tokens, context_size: last_context_size }
}

// ─── GET /sessions/:slug/:id/stats ───────────────────────────────────────────

pub async fn get_session_stats(
    State(app): State<AppState>,
    Path((slug, id)): Path<(String, String)>,
) -> Result<Json<SessionStats>, (StatusCode, String)> {
    let file_path = app.config.server.sessions_dir.join(&slug).join(format!("{id}.jsonl"));
    if !file_path.exists() {
        return Err((StatusCode::NOT_FOUND, format!("Session not found: {id}")));
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let modified_at = fs::metadata(&file_path).ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let active = modified_at.map(|t| now_secs - t < 300).unwrap_or(false);

    let file = fs::File::open(&file_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut title = String::new();
    let mut cwd: Option<String> = None;
    let mut models_used = std::collections::HashSet::new();
    let mut model: Option<String> = None;
    let mut user_turns = 0;
    let mut assistant_turns = 0;
    let mut total_output: u64 = 0;
    let mut context_size: u64 = 0;
    let mut tools: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut files_read: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut files_edited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut started_at: Option<u64> = None;

    for line in BufReader::new(file).lines().flatten() {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
        let entry_type = entry["type"].as_str().unwrap_or("");

        match entry_type {
            "system" => {
                if cwd.is_none() {
                    cwd = entry["cwd"].as_str().map(|s| s.to_string());
                }
                if started_at.is_none() {
                    if let Some(ts) = entry["timestamp"].as_str() {
                        // Parse ISO timestamp to unix
                        started_at = chrono::DateTime::parse_from_rfc3339(ts)
                            .ok()
                            .map(|dt| dt.timestamp() as u64);
                    }
                }
            }
            "user" => {
                let content = &entry["message"]["content"];
                if let Value::String(s) = content {
                    user_turns += 1;
                    if title.is_empty() {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() && !trimmed.starts_with("<command") && !trimmed.starts_with("/init") {
                            title = trimmed.chars().take(80).collect();
                        }
                    }
                }
            }
            "assistant" => {
                assistant_turns += 1;
                let msg = &entry["message"];
                let usage = &msg["usage"];
                if let Some(out) = usage["output_tokens"].as_u64() {
                    total_output += out;
                }
                let input = usage["input_tokens"].as_u64().unwrap_or(0);
                let cache = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                if input + cache > 0 {
                    context_size = input + cache;
                }
                if let Some(m) = msg["model"].as_str() {
                    models_used.insert(m.to_string());
                    model = Some(m.to_string());
                }
                // Extract tool usage and file paths
                if let Some(content) = msg["content"].as_array() {
                    for block in content {
                        if block["type"].as_str() == Some("tool_use") {
                            let name = block["name"].as_str().unwrap_or("?").to_string();
                            *tools.entry(name.clone()).or_insert(0) += 1;
                            let inp = &block["input"];
                            match name.as_str() {
                                "Read" => {
                                    if let Some(p) = inp["file_path"].as_str() {
                                        files_read.insert(p.to_string());
                                    }
                                }
                                "Edit" | "Write" => {
                                    if let Some(p) = inp["file_path"].as_str() {
                                        files_edited.insert(p.to_string());
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if title.is_empty() { title = "(no title)".into(); }

    // Generate suggestions for active sessions
    let mut suggestions = Vec::new();
    if active {
        let ctx_pct = (context_size as f64 / 200_000.0) * 100.0;
        if ctx_pct > 80.0 {
            suggestions.push(format!("Context at {ctx_pct:.0}% — consider /compact"));
        } else if ctx_pct > 50.0 {
            suggestions.push(format!("Context at {ctx_pct:.0}% — still has room"));
        }
        if let Some(start) = started_at {
            let hours = (now_secs - start) as f64 / 3600.0;
            if hours > 4.0 {
                suggestions.push(format!("Session running {hours:.1}h — context may be stale"));
            }
        }
        // Check for heavily-edited files
        let total_tool_calls: usize = tools.values().sum();
        let edit_count = *tools.get("Edit").unwrap_or(&0);
        if edit_count > 20 {
            suggestions.push(format!("{edit_count} edits this session — high churn"));
        }
        if total_tool_calls > 100 {
            suggestions.push(format!("{total_tool_calls} tool calls — busy session"));
        }
    }

    let mut models_vec: Vec<String> = models_used.into_iter().collect();
    models_vec.sort();

    let mut fr: Vec<String> = files_read.into_iter().collect();
    fr.sort();
    let mut fe: Vec<String> = files_edited.into_iter().collect();
    fe.sort();

    Ok(Json(SessionStats {
        session_id: id,
        title,
        cwd,
        model,
        models_used: models_vec,
        active,
        user_turns,
        assistant_turns,
        total_output_tokens: total_output,
        context_size,
        tools,
        files_read: fr,
        files_edited: fe,
        started_at,
        last_activity: modified_at,
        suggestions,
    }))
}

// ─── GET /billing ─────────────────────────────────────────────────────────────

pub async fn get_billing(
    State(app): State<AppState>,
) -> Result<Json<BillingResponse>, (StatusCode, String)> {
    let sessions_dir = &app.config.server.sessions_dir;
    let now = chrono::Local::now();
    let today_str = now.format("%Y-%m-%d").to_string();

    // Find last Thursday
    let weekday = now.weekday().num_days_from_monday(); // Mon=0, Thu=3
    let days_since_thu = ((weekday as i64 - 3) + 7) % 7;
    let last_thu = (now - chrono::Duration::days(days_since_thu))
        .format("%Y-%m-%d").to_string();
    let last_thu_ts = (now - chrono::Duration::days(days_since_thu))
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp() as u64;

    let mut daily_map: std::collections::HashMap<String, (u64, usize)> = std::collections::HashMap::new();

    // Scan all session slugs
    let Ok(entries) = fs::read_dir(sessions_dir) else {
        return Ok(Json(BillingResponse {
            today: BillingPeriod { output_tokens: 0, sessions: 0 },
            billing_cycle: BillingCycle { start: last_thu.clone(), output_tokens: 0, sessions: 0 },
            daily: Vec::new(),
        }));
    };

    for slug_entry in entries.flatten() {
        let slug_path = slug_entry.path();
        if !slug_path.is_dir() { continue; }

        let Ok(files) = fs::read_dir(&slug_path) else { continue };
        for file_entry in files.flatten() {
            let path = file_entry.path();
            if path.extension().and_then(|x| x.to_str()) != Some("jsonl") { continue; }

            let modified = file_entry.metadata().ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            // Skip sessions not modified since last Thursday
            if modified < last_thu_ts { continue; }

            // Quick scan for output tokens
            let mut session_output: u64 = 0;
            let mut session_date = String::new();

            if let Ok(file) = fs::File::open(&path) {
                for line in BufReader::new(file).lines().flatten() {
                    let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
                    let entry_type = entry["type"].as_str().unwrap_or("");
                    if entry_type == "assistant" {
                        if let Some(out) = entry["message"]["usage"]["output_tokens"].as_u64() {
                            session_output += out;
                        }
                    }
                    if entry_type == "system" && session_date.is_empty() {
                        if let Some(ts) = entry["timestamp"].as_str() {
                            session_date = ts.get(..10).unwrap_or("").to_string();
                        }
                    }
                }
            }

            if session_date.is_empty() {
                // Fallback to modified date
                let dt = chrono::DateTime::from_timestamp(modified as i64, 0)
                    .unwrap_or_default();
                session_date = dt.format("%Y-%m-%d").to_string();
            }

            let entry = daily_map.entry(session_date).or_insert((0, 0));
            entry.0 += session_output;
            entry.1 += 1;
        }
    }

    // Build response
    let today_data = daily_map.get(&today_str).copied().unwrap_or((0, 0));
    let cycle_output: u64 = daily_map.values().map(|(t, _)| *t).sum();
    let cycle_sessions: usize = daily_map.values().map(|(_, s)| *s).sum();

    let mut daily: Vec<DailyUsage> = daily_map.into_iter()
        .map(|(date, (output_tokens, sessions))| DailyUsage { date, output_tokens, sessions })
        .collect();
    daily.sort_by(|a, b| b.date.cmp(&a.date));

    Ok(Json(BillingResponse {
        today: BillingPeriod { output_tokens: today_data.0, sessions: today_data.1 },
        billing_cycle: BillingCycle {
            start: last_thu,
            output_tokens: cycle_output,
            sessions: cycle_sessions,
        },
        daily,
    }))
}
