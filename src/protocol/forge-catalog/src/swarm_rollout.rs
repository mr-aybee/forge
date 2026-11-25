//! Deployment strategies on Docker Swarm (D-156).
//!
//! Forge's strategies map onto Swarm like this:
//!
//! | Strategy | How |
//! | --- | --- |
//! | Recreate | The service's own update: every task at once, stop first |
//! | Rolling | The service's own update: `batch_size` tasks at a time, start or stop first, rolled back by Swarm on failure |
//! | Blue-Green | A second service (the other slot) at full size on its own ports; once healthy, Forge's edge moves all traffic to it, and the old slot is scaled to zero (kept for an instant rollback) |
//! | Canary | The other slot grows step by step while the edge moves the same share of traffic to it; then it takes all traffic and the old slot is scaled to zero |
//! | Manual | Not available: Swarm cannot hold an update between batches |
//!
//! Recreate and Rolling run the unslotted service (`""`). Blue-Green and
//! Canary alternate between slots `a` and `b`, and need a site in front of
//! the service so the edge can weight traffic between them.

use serde::{Deserialize, Serialize};

use crate::deploy::{RolloutPolicy, Strategy};
use crate::swarm_service::{FailureAction, UpdateOrder, UpdatePolicy};

/// One step of a Swarm rollout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum SwarmStep {
    /// Create or update the slot's service with the new release.
    Deploy { slot: String, replicas: u32 },
    /// Wait until the slot runs all its tasks, healthy.
    Gate { slot: String },
    /// Change a slot's replica count (the new slot grows, the old shrinks).
    Scale { slot: String, replicas: u32 },
    /// Traffic at the edge: `weight` percent to `slot`, the rest to `other`.
    Route { slot: String, weight: u32, other: String },
    /// Hold, watching for failed tasks.
    Wait { secs: u64 },
    /// Hold until someone approves.
    Approve { reason: String },
}

impl SwarmStep {
    /// The step for people.
    #[must_use]
    pub fn describe(&self) -> String {
        let name = |s: &str| if s.is_empty() { "the service".to_owned() } else { format!("slot {s}") };

        match self {
            Self::Deploy { slot, replicas } => format!("Deploy the release to {} ({replicas} tasks)", name(slot)),
            Self::Gate { slot } => format!("Wait until {} runs all its tasks, healthy", name(slot)),
            Self::Scale { slot, replicas } => format!("Scale {} to {replicas} tasks", name(slot)),
            Self::Route { slot, weight, other } => {
                format!("Send {weight}% of traffic to {} and {}% to {}", name(slot), 100 - weight, name(other))
            }
            Self::Wait { secs } => format!("Watch for {secs} s"),
            Self::Approve { reason } => format!("Wait for approval: {reason}"),
        }
    }
}

/// A Swarm rollout: the steps, and the Swarm update behaviour of the
/// deployed service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmPlan {
    pub strategy: Strategy,
    /// The slot that serves when the rollout succeeded.
    pub slot: String,
    /// The slot that served before (`None`: nothing served yet).
    pub from_slot: Option<String>,
    pub update: UpdatePolicy,
    pub steps: Vec<SwarmStep>,
    /// No task serves for a while: a recreate, or a stop-first update whose
    /// batch is every replica (accepted only with `allowDowntime`).
    #[serde(default)]
    pub downtime: bool,
}

/// What the planner needs to know.
#[derive(Debug, Clone)]
pub struct SwarmPlanInput<'a> {
    pub policy: &'a RolloutPolicy,
    /// Tasks the service runs (replicated mode).
    pub replicas: u32,
    /// The slot serving now: `""` (unslotted), `a`, `b`, or `None` before
    /// the first deployment.
    pub serving: Option<&'a str>,
    /// A site routes traffic to the service through Forge's edge.
    pub routed: bool,
}

fn other_slot(serving: Option<&str>) -> &'static str {
    if serving == Some("a") { "b" } else { "a" }
}

/// Tasks for `weight` percent of `total`, at least one.
fn share(total: u32, weight: u32) -> u32 {
    (total * weight.min(100)).div_ceil(100).max(1)
}

/// Plans a rollout. Fails for strategies Swarm cannot carry out.
pub fn plan(input: &SwarmPlanInput<'_>) -> Result<SwarmPlan, String> {
    let p = input.policy;
    let r = input.replicas.max(1);
    let failure_action = if p.auto_rollback { FailureAction::Rollback } else { FailureAction::Pause };
    let mut update = UpdatePolicy {
        parallelism: p.batch_size.max(1),
        delay_secs: 0,
        order: match p.replacement {
            crate::deploy::Replacement::StartFirst => UpdateOrder::StartFirst,
            crate::deploy::Replacement::StopFirst => UpdateOrder::StopFirst,
        },
        failure_action,
        monitor_secs: u32::try_from(p.stabilization_secs.clamp(5, 600)).unwrap_or(10),
        max_failure_ratio: 0.0,
    };
    let approve = |steps: &mut Vec<SwarmStep>| {
        if p.approval {
            steps.push(SwarmStep::Approve { reason: "before traffic moves".into() });
        }
    };
    let stabilize = p.stabilization_secs.max(10);
    let from_slot = input.serving.map(str::to_owned);

    match p.strategy {
        Strategy::Manual => {
            Err("manual rollouts are not available on Docker Swarm: Swarm cannot hold an update between batches; use Rolling with approval, or Canary".into())
        }
        Strategy::Recreate | Strategy::Rolling => {
            if input.serving.is_some_and(|s| !s.is_empty()) {
                return Err("this service runs in slots (Blue-Green or Canary); keep that strategy".into());
            }

            let downtime = if p.strategy == Strategy::Recreate {
                update.parallelism = 0;
                update.order = UpdateOrder::StopFirst;
                true
            } else if update.order == UpdateOrder::StopFirst && update.parallelism >= r && input.serving.is_some() {
                // As on nodes: stop-first over every running copy at once.
                if !p.allow_downtime {
                    return Err(format!(
                        "stop-first with {} would stop every running task at once; run more replicas, lower \
                         batchSize, or set strategy.allowDowntime: true",
                        if r == 1 { "one replica".to_owned() } else { format!("batches of {}", update.parallelism) }
                    ));
                }

                true
            } else {
                false
            };

            let mut steps = Vec::new();
            approve(&mut steps);
            steps.push(SwarmStep::Deploy { slot: String::new(), replicas: r });
            steps.push(SwarmStep::Gate { slot: String::new() });
            Ok(SwarmPlan { strategy: p.strategy, slot: String::new(), from_slot, update, steps, downtime })
        }
        Strategy::BlueGreen | Strategy::Canary => {
            if !input.routed {
                return Err("Blue-Green and Canary need a site in front of the service, so Forge's edge can move traffic between the slots".into());
            }

            if input.serving == Some("") {
                return Err("this service runs unslotted (Rolling or Recreate); keep that strategy".into());
            }

            let new = other_slot(input.serving).to_owned();
            let old = input.serving.map(str::to_owned);
            let mut steps = Vec::new();

            if p.strategy == Strategy::BlueGreen || old.is_none() {
                steps.push(SwarmStep::Deploy { slot: new.clone(), replicas: r });
                steps.push(SwarmStep::Gate { slot: new.clone() });
                approve(&mut steps);
            } else {
                let weights: Vec<u32> = if p.steps.is_empty() {
                    vec![10, 50]
                } else {
                    p.steps.iter().map(|s| s.weight.clamp(1, 99)).collect()
                };

                for (i, w) in weights.iter().enumerate() {
                    if i == 0 {
                        steps.push(SwarmStep::Deploy { slot: new.clone(), replicas: share(r, *w) });
                    } else {
                        steps.push(SwarmStep::Scale { slot: new.clone(), replicas: share(r, *w) });
                    }

                    steps.push(SwarmStep::Gate { slot: new.clone() });

                    if i == 0 {
                        approve(&mut steps);
                    }

                    if let Some(o) = &old {
                        steps.push(SwarmStep::Route { slot: new.clone(), weight: *w, other: o.clone() });
                    }

                    let pause = p.steps.get(i).map_or(stabilize, |s| s.pause.0.max(10));
                    steps.push(SwarmStep::Wait { secs: pause });

                    if p.steps.get(i).is_some_and(|s| s.approval) {
                        steps.push(SwarmStep::Approve { reason: format!("at {w}% of traffic") });
                    }
                }

                steps.push(SwarmStep::Scale { slot: new.clone(), replicas: r });
                steps.push(SwarmStep::Gate { slot: new.clone() });
            }

            steps.push(SwarmStep::Route { slot: new.clone(), weight: 100, other: old.clone().unwrap_or_default() });
            steps.push(SwarmStep::Wait { secs: stabilize });

            if let Some(o) = old {
                steps.push(SwarmStep::Scale { slot: o, replicas: 0 });
            }

            Ok(SwarmPlan { strategy: p.strategy, slot: new, from_slot, update, steps, downtime: false })
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::deploy::{CanaryStep, Secs};

    fn policy(strategy: Strategy) -> RolloutPolicy {
        RolloutPolicy { strategy, ..RolloutPolicy::default() }
    }

    fn input<'a>(p: &'a RolloutPolicy, serving: Option<&'a str>, routed: bool) -> SwarmPlanInput<'a> {
        SwarmPlanInput { policy: p, replicas: 4, serving, routed }
    }

    #[test]
    fn rolling_and_recreate_update_the_service_in_place() {
        let mut p = policy(Strategy::Rolling);
        p.batch_size = 2;
        p.replacement = crate::deploy::Replacement::StartFirst;
        let plan = plan(&input(&p, None, false)).unwrap();
        assert_eq!(plan.slot, "");
        assert_eq!(plan.update.parallelism, 2);
        assert_eq!(plan.update.order, UpdateOrder::StartFirst);
        assert_eq!(
            plan.steps,
            vec![SwarmStep::Deploy { slot: String::new(), replicas: 4 }, SwarmStep::Gate { slot: String::new() }]
        );
        let plan = super::plan(&input(&policy(Strategy::Recreate), Some(""), false)).unwrap();
        assert_eq!((plan.update.parallelism, plan.update.order), (0, UpdateOrder::StopFirst));
    }

    #[test]
    fn stop_first_over_every_replica_needs_allow_downtime() {
        let mut p = policy(Strategy::Rolling);
        p.replacement = crate::deploy::Replacement::StopFirst;
        p.batch_size = 4;
        p.allow_downtime = false; // as a deployment document defaults it

        // Nothing serves yet: a first deploy has no running copy to lose.
        assert!(!plan(&input(&p, None, false)).unwrap().downtime);
        let e = plan(&input(&p, Some(""), false)).unwrap_err();
        assert!(e.contains("allowDowntime"), "{e}");
        p.allow_downtime = true;
        assert!(plan(&input(&p, Some(""), false)).unwrap().downtime);

        // Smaller batches keep tasks serving.
        p.batch_size = 1;
        p.allow_downtime = false;
        assert!(!plan(&input(&p, Some(""), false)).unwrap().downtime);
        assert!(plan(&input(&policy(Strategy::Recreate), Some(""), false)).unwrap().downtime);
    }

    #[test]
    fn manual_is_refused_and_slotted_strategies_need_a_site() {
        assert!(plan(&input(&policy(Strategy::Manual), None, true)).is_err());
        assert!(plan(&input(&policy(Strategy::BlueGreen), None, false)).is_err());
        assert!(plan(&input(&policy(Strategy::Canary), Some("a"), false)).is_err());
    }

    #[test]
    fn blue_green_switches_slots_and_keeps_the_old_one_at_zero() {
        let p = policy(Strategy::BlueGreen);
        let plan = plan(&input(&p, Some("a"), true)).unwrap();
        assert_eq!(plan.slot, "b");
        assert_eq!(plan.steps.first(), Some(&SwarmStep::Deploy { slot: "b".into(), replicas: 4 }));
        assert!(plan.steps.contains(&SwarmStep::Route { slot: "b".into(), weight: 100, other: "a".into() }));
        assert_eq!(plan.steps.last(), Some(&SwarmStep::Scale { slot: "a".into(), replicas: 0 }));
    }

    #[test]
    fn canary_grows_the_new_slot_with_its_traffic_share() {
        let mut p = policy(Strategy::Canary);
        p.steps = vec![
            CanaryStep { weight: 25, pause: Secs(60), approval: true, analysis: None },
            CanaryStep { weight: 50, pause: Secs(30), approval: false, analysis: None },
        ];
        let plan = plan(&input(&p, Some("b"), true)).unwrap();
        assert_eq!(plan.slot, "a");
        assert_eq!(plan.steps[0], SwarmStep::Deploy { slot: "a".into(), replicas: 1 });
        assert!(plan.steps.contains(&SwarmStep::Route { slot: "a".into(), weight: 25, other: "b".into() }));
        assert!(plan.steps.contains(&SwarmStep::Approve { reason: "at 25% of traffic".into() }));
        assert!(plan.steps.contains(&SwarmStep::Scale { slot: "a".into(), replicas: 2 }));
        assert_eq!(plan.steps.last(), Some(&SwarmStep::Scale { slot: "b".into(), replicas: 0 }));

        for s in &plan.steps {
            assert!(!s.describe().is_empty());
        }
    }

    #[test]
    fn strategies_do_not_switch_between_slotted_and_unslotted() {
        assert!(plan(&input(&policy(Strategy::Rolling), Some("a"), true)).is_err());
        assert!(plan(&input(&policy(Strategy::Canary), Some(""), true)).is_err());
    }
}
