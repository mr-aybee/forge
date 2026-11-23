//! `diagnostics.sleep`: three timed steps. Used to verify durable resume
//! (kill the control plane mid-job, restart, watch it continue) and by the
//! fleet benchmarks.
//!
//! Payload: `{"step_ms": 1000}` (capped at 60 s per step).

use std::time::Duration;

use async_trait::async_trait;
use forge_core::Action;
use forge_jobs::{JobHandler, StepContext, StepError};
use serde_json::{Value, json};

#[derive(Debug, Default)]
pub struct DiagnosticsJob;

#[async_trait]
impl JobHandler for DiagnosticsJob {
    fn kind(&self) -> &'static str {
        "diagnostics.sleep"
    }

    fn steps(&self) -> &'static [&'static str] {
        &["first", "second", "third"]
    }

    fn required_action(&self, _payload: &Value) -> Action {
        Action::JobCreate
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        let ms = ctx.job.payload.get("step_ms").and_then(Value::as_u64).unwrap_or(500).min(60_000);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        ctx.set_state(&format!("step{index}"), json!("done"));
        Ok(())
    }
}
