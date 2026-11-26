//! Compose container states pushed by agents between deploys
//! (`plans/native-mesh.md` 1.8): each stack node's stored rows follow the
//! node's containers, and names follow the rows.

use std::collections::{BTreeMap, HashMap};

use forge_core::ServerId;
use forge_core::ports::TenancyRepository;
use forge_core::stacks::StackRepository;
use serde_json::{Value, json};

use crate::state::AppState;

/// Fields a push owns in a row; the rest (ports, image) comes from the
/// last deploy and stays.
const PUSHED: &[&str] = &["service", "state", "health", "number", "mesh_ip"];

/// The stored rows updated with pushed ones, matched by container name.
/// Containers the push no longer lists are gone and dropped; new ones are
/// added.
#[must_use]
pub fn merge_rows(stored: &Value, pushed: &[Value]) -> Value {
    let old: HashMap<&str, &Value> =
        stored.as_array().into_iter().flatten().filter_map(|r| Some((r["name"].as_str()?, r))).collect();
    let mut out = Vec::with_capacity(pushed.len());

    for p in pushed {
        let Some(name) = p["name"].as_str() else { continue };
        let mut row = old.get(name).map_or_else(|| json!({ "name": name }), |r| (*r).clone());

        for k in PUSHED {
            row[*k] = p.get(*k).cloned().unwrap_or(Value::Null);
        }

        out.push(row);
    }

    Value::Array(out)
}

/// Applies one node's push. Stacks are matched by their label
/// (`<environment slug>/<stack>`); unknown labels are ignored. Pokes the
/// names when anything changed. Errors are logged: a heartbeat never fails
/// on it.
pub async fn ingest(state: &AppState, server: ServerId, states_json: &str) {
    let pushed: BTreeMap<String, Vec<Value>> = match serde_json::from_str(states_json) {
        Ok(p) => p,
        Err(e) => return tracing::debug!(%server, error = %e, "malformed stack states"),
    };

    if let Err(e) = apply(state, server, &pushed).await {
        tracing::warn!(%server, error = %e, "recording stack states failed");
    }
}

async fn apply(
    state: &AppState,
    server: ServerId,
    pushed: &BTreeMap<String, Vec<Value>>,
) -> forge_core::DomainResult<()> {
    let mut changed = false;

    for mut node in state.store.nodes_on_server(server).await? {
        let Some(stack) = state.store.get_stack(node.stack_id).await? else { continue };
        let env = state.store.get_environment(stack.environment_id).await?;
        let label = format!("{}/{}", crate::stacks::ns_slug(&env.name), stack.name);
        let rows = pushed.get(&label).map_or(&[][..], Vec::as_slice);
        let merged = merge_rows(&node.state, rows);

        if merged == node.state {
            continue;
        }

        node.state = merged;
        node.updated_at = state.clock.now();
        state.store.put_node(&node).await?;
        changed = true;
    }

    if changed {
        crate::names::poke();
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Reason: test assertions.
mod tests {
    use super::*;

    #[test]
    fn pushed_rows_replace_state_and_keep_deploy_fields() {
        let stored = json!([
            { "name": "p-api-1", "service": "api", "state": "running", "ports": [8080], "image": "api:1" },
            { "name": "p-api-2", "service": "api", "state": "running" },
        ]);
        let pushed = vec![
            json!({ "name": "p-api-1", "service": "api", "state": "running", "health": "healthy", "number": 1, "mesh_ip": "10.64.12.4" }),
            json!({ "name": "p-api-3", "service": "api", "state": "running", "number": 3 }),
        ];
        let m = merge_rows(&stored, &pushed);
        let rows = m.as_array().unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["image"], "api:1");
        assert_eq!(rows[0]["mesh_ip"], "10.64.12.4");
        assert_eq!(rows[1]["name"], "p-api-3");
        assert!(rows.iter().all(|r| r["name"] != "p-api-2"));
    }
}
