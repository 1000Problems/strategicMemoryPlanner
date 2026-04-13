use serde::Serialize;
use tokio::sync::broadcast;

/// An event emitted when SEP state changes.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum Event {
    #[serde(rename = "decision.new")]
    DecisionNew {
        project: String,
        domain: String,
        decision: String,
    },
    #[serde(rename = "decision.changed")]
    DecisionChanged {
        project: String,
        domain: String,
        old_decision: String,
        new_decision: String,
    },
    #[serde(rename = "phase.changed")]
    PhaseChanged {
        project: String,
        domain: String,
        old_phase: String,
        new_phase: String,
    },
    #[serde(rename = "state.updated")]
    StateUpdated {
        project: String,
    },
    #[serde(rename = "ingestion.complete")]
    IngestionComplete {
        project: String,
        job_id: String,
        raw_tokens: usize,
        digest_tokens: usize,
        decisions_extracted: usize,
    },
}

/// Broadcast bus for SEP events.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn emit(&self, event: Event) {
        let _ = self.tx.send(event); // Ignore if no receivers
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}
