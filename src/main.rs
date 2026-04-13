mod api;
mod config;
mod db;
mod events;
mod ingester;
mod memory;
mod secretary;

use anyhow::{Context, Result};
use axum::{response::IntoResponse, routing::get, routing::post, Router};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use config::Config;
use events::EventBus;
use secretary::prompts::PromptLoader;
use secretary::Secretary;

/// Shared application state available to all route handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub secretary: Arc<dyn Secretary>,
    pub prompts: Arc<PromptLoader>,
    pub events: EventBus,
}

impl AppState {
    /// Open (or create) the project's SQLite database.
    pub fn open_project_db(&self, project: &str) -> Result<Connection> {
        let db_path = self.config.project_db_path(project);
        db::open_db(&db_path)
    }
}

async fn serve_dashboard() -> impl IntoResponse {
    axum::response::Html(include_str!("../static/dashboard.html"))
}

#[tokio::main]
async fn main() -> Result<()> {
    // Init logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "smp=info".into()),
        )
        .init();

    // Load config
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "smp.toml".to_string());
    let config = Config::load(&PathBuf::from(&config_path))
        .with_context(|| format!("Failed to load config from {}", config_path))?;

    tracing::info!(port = config.server.port, "Starting SMP daemon");

    // Ensure data directory exists
    std::fs::create_dir_all(&config.server.data_dir)?;

    // Build Secretary backend
    let secretary = secretary::build_secretary(&config)
        .context("Failed to initialize Secretary backend")?;
    tracing::info!(backend = secretary.name(), "Secretary ready");

    // Load prompt templates
    let prompts = PromptLoader::new(&config.secretary.prompts_dir);

    // Build shared state
    let state = AppState {
        config: Arc::new(config.clone()),
        secretary: Arc::from(secretary),
        prompts: Arc::new(prompts),
        events: EventBus::new(256),
    };

    // Build router
    let app = Router::new()
        // Dashboard
        .route("/", get(serve_dashboard))
        // System
        .route("/health", get(api::system::health))
        // Projects
        .route("/projects", get(api::projects::list_projects))
        .route("/projects", post(api::projects::create_project))
        // Ingestion
        .route("/ingest", post(api::ingest::ingest_log))
        // Memory
        .route("/memory/{project}/hot", get(api::memory::get_hot_memory))
        .route("/memory/{project}/state", get(api::memory::get_state))
        .route("/memory/{project}/brain", get(api::memory::get_brain))
        // Events
        .route("/events/{project}", get(events::sse::event_stream))
        // Debug
        .route("/debug/query", get(api::debug::debug_query))
        .layer(CorsLayer::permissive())
        .with_state(state);

    // Start server
    let addr = format!("0.0.0.0:{}", config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(addr = %addr, "SMP listening");

    axum::serve(listener, app).await?;
    Ok(())
}
