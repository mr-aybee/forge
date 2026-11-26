//! Bounded YAML reading for Forge documents (`forge.deploy.yaml`,
//! `forge.stack.yaml`): YAML 1.2, one document, limits on size, depth,
//! nodes and aliases. Shared by the server and the CLI (feature `yaml`).

use serde_json::Value;

use crate::deploy::MAX_DOC_BYTES;

/// Parses a document's YAML into a [`Value`] within the document bounds.
pub fn parse_document(text: &str) -> Result<Value, String> {
    if text.len() > MAX_DOC_BYTES {
        return Err(format!("the document is larger than {} KiB", MAX_DOC_BYTES / 1024));
    }

    if text.trim().is_empty() {
        return Err("the document is empty".into());
    }

    let mut budget = serde_saphyr::Budget::default();
    budget.max_depth = 32;
    budget.max_nodes = 20_000;
    budget.max_aliases = 200;
    budget.max_anchors = 200;
    budget.max_documents = 1;
    budget.max_events = 100_000;
    budget.max_total_scalar_bytes = MAX_DOC_BYTES * 4;
    budget.max_recorded_anchor_bytes = MAX_DOC_BYTES * 4;
    let mut options = serde_saphyr::Options::default();
    options.budget = Some(budget);
    options.strict_booleans = true;
    options.leading_zero_numbers_as_strings = true;

    serde_saphyr::from_str_with_options(text, options).map_err(|e| {
        let msg = e.to_string();
        format!("invalid YAML: {}", msg.lines().next().unwrap_or(&msg))
    })
}
