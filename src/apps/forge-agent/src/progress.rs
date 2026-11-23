//! Live output of long-running commands (package installs), streamed to the
//! control plane on heartbeats so the console can show it while it runs.
//!
//! The operation id travels as a task-local: [`scope`] wraps the background
//! task that executes a command, and anything below it (the helper client)
//! calls [`push`] without knowing which operation it serves.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use forge_proto::messages::CommandProgress;

tokio::task_local! {
    static OP: String;
}

/// Unsent output per operation; bounded so a chatty recipe cannot grow memory.
static PENDING: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
static RUNNING: AtomicUsize = AtomicUsize::new(0);
const MAX_PENDING: usize = 64 * 1024;

/// Runs `f` as operation `op`: output pushed inside it is attributed to `op`.
pub async fn scope<F: Future>(op: String, f: F) -> F::Output {
    RUNNING.fetch_add(1, Ordering::SeqCst);
    let out = OP.scope(op, f).await;
    RUNNING.fetch_sub(1, Ordering::SeqCst);
    out
}

/// Appends one line of output for the current operation (no-op outside [`scope`]).
pub fn push(line: &str) {
    let Ok(op) = OP.try_with(Clone::clone) else { return };
    let Ok(mut guard) = PENDING.lock() else { return };
    let buf = guard.get_or_insert_with(HashMap::new).entry(op).or_default();
    if buf.len() < MAX_PENDING {
        buf.push_str(line);
        buf.push('\n');
    }
}

/// Takes everything not sent yet.
pub fn drain() -> Vec<CommandProgress> {
    let Ok(mut guard) = PENDING.lock() else { return vec![] };
    guard
        .take()
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, t)| !t.is_empty())
        .map(|(operation_id, text)| CommandProgress { operation_id, text })
        .collect()
}

/// Whether any streaming command is running (heartbeats then come back quickly).
pub fn active() -> bool {
    RUNNING.load(Ordering::SeqCst) > 0
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn output_is_attributed_to_its_operation() {
        push("outside any operation is dropped");
        scope("op-a".into(), async {
            assert!(active());
            push("line 1");
            push("line 2");
        })
        .await;
        let out = drain();
        let a = out.iter().find(|p| p.operation_id == "op-a").unwrap();
        assert_eq!(a.text, "line 1\nline 2\n");
        assert!(drain().is_empty(), "drained once");
    }
}
