use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use forge_core::Action;
use forge_core::model::Job;
use serde_json::Value;

/// Why a step failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StepError {
    /// Transient failure; the job is re-queued with backoff until
    /// `max_attempts` is reached. The same step runs again.
    #[error("retryable: {0}")]
    Retryable(String),
    /// Permanent failure; the job fails immediately.
    #[error("fatal: {0}")]
    Fatal(String),
}

/// Data available to a running step.
#[derive(Debug)]
pub struct StepContext {
    /// Snapshot of the job row at claim time.
    pub job: Job,
    /// Mutable state carried between steps; persisted after each success.
    pub state: Value,
}

impl StepContext {
    /// Reads a string from the job payload.
    #[must_use]
    pub fn payload_str(&self, key: &str) -> Option<&str> {
        self.job.payload.get(key).and_then(Value::as_str)
    }

    /// Stores a value in the carried state.
    pub fn set_state(&mut self, key: &str, value: Value) {
        if !self.state.is_object() {
            self.state = Value::Object(serde_json::Map::new());
        }

        if let Some(obj) = self.state.as_object_mut() {
            obj.insert(key.to_owned(), value);
        }
    }

    /// A value saved in the job's state.
    #[must_use]
    pub fn state_value(&self, key: &str) -> Option<&Value> {
        self.state.get(key)
    }
}

/// Defines one kind of job. Implementations are registered in a
/// [`JobRegistry`]; adding a job kind never touches the engine (open/closed).
#[async_trait]
pub trait JobHandler: Send + Sync + std::fmt::Debug {
    /// Unique kind, e.g. `agent.command`.
    fn kind(&self) -> &'static str;
    /// Ordered step names. Their count must never shrink for queued jobs.
    fn steps(&self) -> &'static [&'static str];
    /// Action re-checked for the job's principal before each step. It may
    /// depend on the payload (e.g. read-only vs mutating agent commands).
    fn required_action(&self, payload: &Value) -> Action;
    /// Runs step `index`. Must be idempotent: it may run again after a crash.
    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError>;
}

/// Kind → handler map.
#[derive(Debug, Default, Clone)]
pub struct JobRegistry {
    handlers: HashMap<&'static str, Arc<dyn JobHandler>>,
}

impl JobRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a handler for its job kind.
    pub fn register(&mut self, handler: Arc<dyn JobHandler>) -> &mut Self {
        self.handlers.insert(handler.kind(), handler);
        self
    }

    /// The handler for a job kind.
    #[must_use]
    pub fn get(&self, kind: &str) -> Option<Arc<dyn JobHandler>> {
        self.handlers.get(kind).cloned()
    }

    /// The registered job kinds.
    pub fn kinds(&self) -> impl Iterator<Item = &&'static str> {
        self.handlers.keys()
    }
}
