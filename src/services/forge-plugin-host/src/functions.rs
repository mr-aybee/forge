use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use forge_core::model::AgentStatus;
use forge_core::ports::{Authorizer, ServerRepository};
use forge_core::{AccessRequest, Action, AuthzContext, EntityKind, EntityRef, Principal};
use forge_sdk::host_functions;
use serde_json::{Value, json};

use crate::runner::HostCalls;

/// Who is invoking which plugin with which approved permissions.
#[derive(Debug, Clone)]
pub struct Invocation {
    pub plugin_id: String,
    pub principal: Principal,
    pub context: AuthzContext,
    pub approved: HashSet<String>,
}

/// Implementations of the `forge:plugin/host` functions.
#[derive(Debug, Clone)]
pub struct ForgeHostFunctions {
    servers: Arc<dyn ServerRepository>,
    authz: Arc<dyn Authorizer>,
}

impl ForgeHostFunctions {
    #[must_use]
    pub fn new(servers: Arc<dyn ServerRepository>, authz: Arc<dyn Authorizer>) -> Self {
        Self { servers, authz }
    }

    #[must_use]
    pub fn bind(&self, invocation: Invocation) -> BoundHostFunctions {
        BoundHostFunctions { functions: self.clone(), invocation }
    }
}

/// Host functions bound to one invocation.
#[derive(Debug)]
pub struct BoundHostFunctions {
    functions: ForgeHostFunctions,
    invocation: Invocation,
}

impl BoundHostFunctions {
    fn require_approved(&self, function: &str) -> Result<(), String> {
        let p = host_functions::permission(function);
        if self.invocation.approved.contains(&p) {
            Ok(())
        } else {
            Err(format!("permission `{p}` not approved for plugin"))
        }
    }

    async fn server_online(&self, args: &Value) -> Result<Value, String> {
        let hostname = args.get("hostname").and_then(Value::as_str).ok_or("missing hostname")?;
        if hostname.len() > 253 {
            return Err("hostname too long".into());
        }

        let candidates = self.functions.servers.list_servers(None).await.map_err(|e| e.to_string())?;

        for s in candidates.into_iter().filter(|s| s.hostname.eq_ignore_ascii_case(hostname)) {
            // The *user's* rights bound what the plugin can see.
            let req = AccessRequest::new(
                self.invocation.principal.clone(),
                Action::View,
                EntityRef::new(EntityKind::Server, s.id),
                self.invocation.context,
            );

            if self.functions.authz.is_allowed(&req).await {
                return Ok(json!(s.status == AgentStatus::Online));
            }
        }

        // Unknown and invisible servers are indistinguishable.
        Ok(json!(false))
    }
}

#[async_trait]
impl HostCalls for BoundHostFunctions {
    async fn call(&self, function: &str, args: Value) -> Result<Value, String> {
        self.require_approved(function)?;

        match function {
            host_functions::LOG => {
                let msg = args.get("message").and_then(Value::as_str).unwrap_or_default();
                tracing::info!(plugin = %self.invocation.plugin_id, message = %msg.chars().take(1024).collect::<String>(), "plugin log");
                Ok(Value::Null)
            }
            host_functions::SERVER_ONLINE => self.server_online(&args).await,
            other => Err(format!("unknown host function `{other}`")),
        }
    }
}
