use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use regex::Regex;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path as FsPath, PathBuf};

use crate::AppState;

// ─── Response Types ───────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ContextTrace {
    pub project: String,
    pub path: String,
    pub chain: Vec<ChainEntry>,
    pub summary: ContextSummary,
}

#[derive(Serialize)]
pub struct ChainEntry {
    pub path: String,
    pub scope: String, // "global" | "user" | "portfolio" | "project"
    pub exists: bool,
    pub token_count: usize,
    pub references: Vec<FileReference>,
}

#[derive(Serialize)]
pub struct FileReference {
    pub path: String,
    pub ref_type: String, // "always" | "conditional" | "external"
    pub condition: Option<String>,
    pub token_count: Option<usize>,
    pub exists: Option<bool>,
    pub note: Option<String>,
}

#[derive(Serialize)]
pub struct ContextSummary {
    pub always_tokens: usize,
    pub conditional_tokens: usize,
    pub total_worst_case: usize,
    pub external_services: usize,
    pub files_missing: usize,
    pub context_window_pct: f64, // percentage of 200k
}

// ─── GET /projects/{name}/context ─────────────────────────────────────────────

pub async fn get_context(
    State(app): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ContextTrace>, (StatusCode, String)> {
    // Look up project path from DB
    let conn = app.open_project_db(&name).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}"))
    })?;

    let project_path: Option<String> = conn
        .prepare("SELECT path FROM projects WHERE name = ?1")
        .and_then(|mut stmt| stmt.query_row([&name], |row| row.get(0)))
        .ok();

    let project_path = project_path.ok_or_else(|| {
        (StatusCode::NOT_FOUND, format!("Project '{name}' has no path set"))
    })?;

    let project_dir = PathBuf::from(&project_path);
    if !project_dir.exists() {
        return Err((StatusCode::NOT_FOUND, format!("Path does not exist: {project_path}")));
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/angel".to_string());
    let mut chain: Vec<ChainEntry> = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();

    // 1. Collect CLAUDE.md chain: walk from project dir up to root
    let mut dir = Some(project_dir.as_path());
    let mut claude_md_paths: Vec<(PathBuf, String)> = Vec::new();

    while let Some(d) = dir {
        let claude_md = d.join("CLAUDE.md");
        let scope = classify_scope(d, &project_dir, &home);
        claude_md_paths.push((claude_md, scope));
        dir = d.parent();
        // Stop at home directory's parent
        if d == FsPath::new(&home).parent().unwrap_or(FsPath::new("/")) {
            break;
        }
    }

    // Also include ~/.claude/CLAUDE.md (global)
    let global_claude = PathBuf::from(&home).join(".claude").join("CLAUDE.md");
    claude_md_paths.push((global_claude, "global".to_string()));

    // Reverse so we go global → portfolio → project
    claude_md_paths.reverse();

    // 2. For each CLAUDE.md, read content, parse references, count tokens
    for (claude_path, scope) in &claude_md_paths {
        let exists = claude_path.exists();
        let content = if exists {
            std::fs::read_to_string(claude_path).unwrap_or_default()
        } else {
            String::new()
        };
        let token_count = estimate_tokens(&content);

        let references = if exists {
            extract_references(&content, &project_dir, &home, &mut seen_paths)
        } else {
            Vec::new()
        };

        // Skip non-existent CLAUDE.md from uninteresting parent dirs
        if !exists && (scope == "parent" || scope == "user") {
            continue;
        }

        let display_path = shorten_path(&claude_path.to_string_lossy(), &home);
        chain.push(ChainEntry {
            path: display_path,
            scope: scope.clone(),
            exists,
            token_count,
            references,
        });
    }

    // 3. Compute summary
    let always_tokens: usize = chain.iter()
        .filter(|e| e.exists)
        .map(|e| e.token_count)
        .sum();

    let conditional_tokens: usize = chain.iter()
        .flat_map(|e| &e.references)
        .filter(|r| r.ref_type == "conditional" || r.ref_type == "always")
        .filter_map(|r| r.token_count)
        .sum();

    let external_services = chain.iter()
        .flat_map(|e| &e.references)
        .filter(|r| r.ref_type == "external")
        .count();

    let files_missing = chain.iter()
        .flat_map(|e| &e.references)
        .filter(|r| r.exists == Some(false))
        .count();

    let total = always_tokens + conditional_tokens;
    let context_window_pct = (total as f64 / 200_000.0) * 100.0;

    let summary = ContextSummary {
        always_tokens,
        conditional_tokens,
        total_worst_case: total,
        external_services,
        files_missing,
        context_window_pct,
    };

    Ok(Json(ContextTrace {
        project: name,
        path: project_path,
        chain,
        summary,
    }))
}

// ─── POST /pick-folder ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct PickFolderResponse {
    pub path: Option<String>,
    pub cancelled: bool,
}

/// Opens the native macOS folder picker dialog via osascript.
pub async fn pick_folder() -> Result<Json<PickFolderResponse>, (StatusCode, String)> {
    let output = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg("POSIX path of (choose folder with prompt \"Select project folder\")")
        .output()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("osascript failed: {e}")))?;

    if !output.status.success() {
        // User cancelled the dialog
        return Ok(Json(PickFolderResponse {
            path: None,
            cancelled: true,
        }));
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // Remove trailing slash that osascript adds
    let path = path.trim_end_matches('/').to_string();

    Ok(Json(PickFolderResponse {
        path: Some(path),
        cancelled: false,
    }))
}

// ─── Reference Extraction ─────────────────────────────────────────────────────

fn extract_references(
    content: &str,
    project_dir: &FsPath,
    home: &str,
    seen: &mut HashSet<String>,
) -> Vec<FileReference> {
    let mut refs: Vec<FileReference> = Vec::new();

    // Pattern 1: Explicit file paths with cat/source/read commands
    // e.g., "cat ~/1000Problems/Skills/shared/frontend-design/SKILL.md"
    // e.g., "source ~/1000Problems/secrets.env"
    let cmd_re = Regex::new(r"(?:cat|source|read)\s+(~/[^\s`\)]+)").unwrap();
    for cap in cmd_re.captures_iter(content) {
        let raw_path = &cap[1];
        let resolved = resolve_path(raw_path, project_dir, home);
        let resolved_str = resolved.to_string_lossy().to_string();
        if seen.contains(&resolved_str) {
            continue;
        }
        seen.insert(resolved_str.clone());

        let exists = resolved.exists();
        let token_count = if exists {
            std::fs::read_to_string(&resolved).ok().map(|c| estimate_tokens(&c))
        } else {
            None
        };

        // Determine if it's conditional based on surrounding context
        let is_conditional = is_conditional_reference(content, raw_path);
        let condition = extract_condition(content, raw_path);

        // Check if it's secrets.env — don't count tokens for that
        let is_secret = raw_path.contains("secrets.env");

        refs.push(FileReference {
            path: shorten_path(&resolved_str, home),
            ref_type: if is_secret {
                "conditional".to_string()
            } else if is_conditional {
                "conditional".to_string()
            } else {
                "always".to_string()
            },
            condition,
            token_count: if is_secret { None } else { token_count },
            exists: Some(exists),
            note: if is_secret { Some("source only, never cat".to_string()) } else { None },
        });
    }

    // Pattern 2: "read BUILD.md" / "read SPEC.md" / "read DESIGN.md" etc.
    let read_re = Regex::new(r"(?i)(?:read|check|consult)\s+(?:the\s+)?(?:project's?\s+)?(\w+\.md)\b").unwrap();
    for cap in read_re.captures_iter(content) {
        let filename = &cap[1];
        // Skip CLAUDE.md — that's the chain itself
        if filename.eq_ignore_ascii_case("CLAUDE.md") {
            continue;
        }
        let resolved = project_dir.join(filename);
        let resolved_str = resolved.to_string_lossy().to_string();
        if seen.contains(&resolved_str) {
            continue;
        }
        seen.insert(resolved_str.clone());

        let exists = resolved.exists();
        let token_count = if exists {
            std::fs::read_to_string(&resolved).ok().map(|c| estimate_tokens(&c))
        } else {
            None
        };

        let condition = extract_condition(content, filename);

        refs.push(FileReference {
            path: shorten_path(&resolved_str, home),
            ref_type: "conditional".to_string(),
            condition,
            token_count,
            exists: Some(exists),
            note: None,
        });
    }

    // Pattern 3: TASK-*.md glob
    let task_re = Regex::new(r"TASK-\*\.md|TASK.+\.md").unwrap();
    if task_re.is_match(content) {
        // Glob for actual TASK files in project dir
        if let Ok(entries) = std::fs::read_dir(project_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.starts_with("TASK") && fname.ends_with(".md") {
                    let resolved = project_dir.join(&fname);
                    let resolved_str = resolved.to_string_lossy().to_string();
                    if seen.contains(&resolved_str) {
                        continue;
                    }
                    seen.insert(resolved_str.clone());

                    let token_count = std::fs::read_to_string(&resolved)
                        .ok()
                        .map(|c| estimate_tokens(&c));

                    refs.push(FileReference {
                        path: shorten_path(&resolved_str, home),
                        ref_type: "conditional".to_string(),
                        condition: Some("if executing a TASK spec".to_string()),
                        token_count,
                        exists: Some(true),
                        note: None,
                    });
                }
            }
        }
    }

    // Pattern 4: External services — localhost:PORT
    let port_re = Regex::new(r"localhost:(\d+)").unwrap();
    let mut seen_ports: HashSet<String> = HashSet::new();
    for cap in port_re.captures_iter(content) {
        let port = &cap[1];
        if seen_ports.contains(port) {
            continue;
        }
        seen_ports.insert(port.to_string());

        // Try to identify the service from surrounding text
        let service_name = identify_service(content, port);

        refs.push(FileReference {
            path: format!("localhost:{port}"),
            ref_type: "external".to_string(),
            condition: None,
            token_count: None,
            exists: None,
            note: Some(service_name),
        });
    }

    // Pattern 5: External URLs (https://...)
    let url_re = Regex::new(r#"https://[^\s\)"']+"#).unwrap();
    let mut seen_urls: HashSet<String> = HashSet::new();
    for m in url_re.find_iter(content) {
        let url = m.as_str().trim_end_matches(|c: char| c == ',' || c == '.' || c == '\'');
        if seen_urls.contains(url) {
            continue;
        }
        seen_urls.insert(url.to_string());

        // Skip common non-service URLs (docs, github, etc.)
        if url.contains("github.com") || url.contains("claude.ai") || url.contains("fonts.google") {
            continue;
        }

        refs.push(FileReference {
            path: url.to_string(),
            ref_type: "external".to_string(),
            condition: None,
            token_count: None,
            exists: None,
            note: Some("API endpoint".to_string()),
        });
    }

    refs
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn estimate_tokens(text: &str) -> usize {
    // chars/4 is a reasonable approximation for English text
    text.len() / 4
}

fn resolve_path(raw: &str, project_dir: &FsPath, home: &str) -> PathBuf {
    if raw.starts_with("~/") {
        PathBuf::from(home).join(&raw[2..])
    } else if raw.starts_with('/') {
        PathBuf::from(raw)
    } else {
        project_dir.join(raw)
    }
}

fn shorten_path(path: &str, home: &str) -> String {
    if path.starts_with(home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    }
}

fn classify_scope(dir: &FsPath, project_dir: &FsPath, home: &str) -> String {
    let home_path = FsPath::new(home);
    if dir == project_dir {
        "project".to_string()
    } else if dir.starts_with(home_path.join("1000Problems")) && dir != project_dir {
        "portfolio".to_string()
    } else if dir == home_path {
        "user".to_string()
    } else {
        "parent".to_string()
    }
}

fn is_conditional_reference(content: &str, path_fragment: &str) -> bool {
    // Look for conditional language near the path reference
    let lower = content.to_lowercase();
    let idx = lower.find(&path_fragment.to_lowercase()).unwrap_or(0);
    let context_start = idx.saturating_sub(300);
    let context_end = (idx + path_fragment.len() + 100).min(lower.len());
    let context = &lower[context_start..context_end];

    context.contains("before implement")
        || context.contains("before building")
        || context.contains("before any")
        || context.contains("if ")
        || context.contains("when you")
        || context.contains("only when")
        || context.contains("before doing")
        || context.contains("before making")
        || context.contains("mandatory for")
}

fn extract_condition(content: &str, path_fragment: &str) -> Option<String> {
    let lower = content.to_lowercase();
    let idx = lower.find(&path_fragment.to_lowercase())?;
    let context_start = idx.saturating_sub(80);
    let context_end = (idx + path_fragment.len() + 40).min(content.len());
    let context = &content[context_start..context_end];

    // Extract the nearest sentence/clause
    let sentence = context.lines()
        .find(|line| line.to_lowercase().contains(&path_fragment.to_lowercase()))
        .unwrap_or(context);

    let trimmed = sentence.trim();
    if trimmed.len() > 5 && trimmed.len() < 120 {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn identify_service(content: &str, port: &str) -> String {
    let lower = content.to_lowercase();
    let idx = lower.find(&format!("localhost:{port}")).unwrap_or(0);
    let start = idx.saturating_sub(60);
    let end = (idx + 20).min(lower.len());
    let context = &lower[start..end];

    if context.contains("lightrag") || context.contains("rag") {
        "LightRAG knowledge graph".to_string()
    } else if context.contains("ollama") {
        "Ollama LLM".to_string()
    } else if context.contains("smp") || port == "19800" {
        "SMP daemon".to_string()
    } else if context.contains("mcp") {
        "MCP server".to_string()
    } else {
        format!("service on port {port}")
    }
}

// ─── GET /read-file ───────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct ReadFileParams {
    pub path: String,
}

#[derive(Serialize)]
pub struct ReadFileResponse {
    pub content: String,
    pub token_count: usize,
    pub exists: bool,
}

/// Read a file's content for inline display in the context viewer.
/// Only allows reading .md, .txt, .toml files under known safe directories.
pub async fn read_file(
    axum::extract::Query(params): axum::extract::Query<ReadFileParams>,
) -> Result<Json<ReadFileResponse>, (StatusCode, String)> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/angel".to_string());

    // Resolve ~ in path
    let resolved = if params.path.starts_with("~/") {
        PathBuf::from(&home).join(&params.path[2..])
    } else {
        PathBuf::from(&params.path)
    };

    // Security: only allow reading files under home directory
    let home_path = PathBuf::from(&home);
    if !resolved.starts_with(&home_path) {
        return Err((StatusCode::FORBIDDEN, "Can only read files under home directory".to_string()));
    }

    // Security: only allow certain extensions
    let ext = resolved.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !["md", "txt", "toml", "sql", "env"].contains(&ext) {
        return Err((StatusCode::FORBIDDEN, format!("File type .{ext} not allowed")));
    }

    if !resolved.exists() {
        return Ok(Json(ReadFileResponse {
            content: String::new(),
            token_count: 0,
            exists: false,
        }));
    }

    let content = std::fs::read_to_string(&resolved).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Read error: {e}"))
    })?;

    let token_count = estimate_tokens(&content);

    Ok(Json(ReadFileResponse {
        content,
        token_count,
        exists: true,
    }))
}
