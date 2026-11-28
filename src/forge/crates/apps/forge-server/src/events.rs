//! Live events pushed to UI clients over `/api/v1/events`.

use forge_core::ServerId;
use forge_core::model::AgentStatus;
use forge_jobs::JobEvent;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Job(JobEvent),
    Server { server_id: ServerId, status: AgentStatus },
}

impl ServerEvent {
    #[must_use]
    pub fn server(server_id: ServerId, status: AgentStatus) -> Self {
        Self::Server { server_id, status }
    }
}
