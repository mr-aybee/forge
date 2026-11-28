//! Container log batches already stored, so a batch sent again (its ack was
//! lost because forge-server gave up waiting) is acknowledged without being
//! written twice. Bounded: the oldest entries are forgotten first. Kept in
//! memory, so a worker restart between storing and acknowledging can still
//! store one batch twice; the agent spool (file ids) closes that later.

use std::collections::{HashSet, VecDeque};

use forge_core::ServerId;
use forge_proto::messages::ContainerLogBatch;

/// Batches remembered.
pub const CAPACITY: usize = 8_192;

/// What identifies a batch. `seq` restarts with the node's helper, so the
/// batch's shape is part of the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BatchKey {
    server: ServerId,
    seq: u64,
    lines: usize,
    first_ts: i64,
    last_ts: i64,
    dropped: u64,
    bytes: usize,
}

impl BatchKey {
    #[must_use]
    pub fn of(server: ServerId, b: &ContainerLogBatch) -> Self {
        Self {
            server,
            seq: b.seq,
            lines: b.lines.len(),
            first_ts: b.lines.first().map_or(0, |l| l.ts_ms),
            last_ts: b.lines.last().map_or(0, |l| l.ts_ms),
            dropped: b.dropped,
            bytes: b.lines.iter().map(|l| l.line.len()).sum(),
        }
    }
}

/// A bounded set of stored batches, oldest forgotten first.
#[derive(Debug, Default)]
pub struct Stored {
    order: VecDeque<BatchKey>,
    keys: HashSet<BatchKey>,
}

impl Stored {
    #[must_use]
    pub fn contains(&self, k: &BatchKey) -> bool {
        self.keys.contains(k)
    }

    pub fn insert(&mut self, k: BatchKey) {
        if !self.keys.insert(k) {
            return;
        }

        self.order.push_back(k);

        while self.order.len() > CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.keys.remove(&old);
            }
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_a_bounded_number_of_batches() {
        let node = ServerId::new();
        let mut s = Stored::default();
        let key = |seq| BatchKey::of(node, &ContainerLogBatch { seq, lines: vec![], dropped: 0 });

        for seq in 0..(CAPACITY as u64 + 10) {
            s.insert(key(seq));
        }

        assert_eq!(s.len(), CAPACITY);
        assert!(!s.contains(&key(0)), "the oldest are forgotten");
        assert!(s.contains(&key(CAPACITY as u64 + 9)));
        let other = BatchKey::of(ServerId::new(), &ContainerLogBatch { seq: 5, lines: vec![], dropped: 0 });
        assert!(!s.contains(&other), "per node");
    }
}
