use axum::extract::{Path, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::Stream;
use std::convert::Infallible;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::emitter::Event;
use crate::AppState;

/// SSE endpoint: GET /events/{project}
/// Streams real-time events filtered by project.
pub async fn event_stream(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.events.subscribe();

    let stream = BroadcastStream::new(rx)
        .filter_map(move |result| {
            match result {
                Ok(event) => {
                    // Filter events to this project
                    let event_project = match &event {
                        Event::DecisionNew { project, .. } => project,
                        Event::DecisionChanged { project, .. } => project,
                        Event::PhaseChanged { project, .. } => project,
                        Event::StateUpdated { project } => project,
                        Event::IngestionComplete { project, .. } => project,
                    };

                    if *event_project == project {
                        let json = serde_json::to_string(&event).unwrap_or_default();
                        Some(Ok(SseEvent::default().data(json)))
                    } else {
                        None
                    }
                }
                Err(_) => None, // Lagged — skip
            }
        });

    Sse::new(stream).keep_alive(KeepAlive::default())
}
