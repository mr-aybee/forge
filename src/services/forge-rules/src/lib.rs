//! Rule evaluation shared by alerts (#11), automation event rules (#8) and
//! node alarms (#14). Pure: no I/O, no clock of its own, no Forge types.
//!
//! A [`RuleSpec`] says when a condition holds (the trigger and its [`Mode`])
//! and when it is over (the [`Recovery`]). A [`Tracker`] holds one series'
//! state: feed it [`Sample`]s in time order with [`Tracker::observe`] and
//! call [`Tracker::tick`] on a timer; each returns the [`Event`]s that
//! happened. The tracker is serializable, so an evaluator that restarts
//! keeps its timers instead of silently resetting them.
//!
//! Semantics (issue #11, section 2):
//!
//! * Every sample is evidence only until its `valid_until`. Time not covered
//!   by valid evidence is **unknown**: it is neither a breach nor a
//!   recovery. Unknown interrupts a pending condition and a pending
//!   recovery; a firing condition stays firing until valid recovery
//!   evidence returns.
//! * Samples must arrive in time order. A sample at or before the last one
//!   is a duplicate or late and is ignored; one too far in the future is
//!   refused. Both are reported, never applied.
//! * Recovery is separate from the trigger (hysteresis): "CPU > 85 % for 30
//!   minutes" fires, "CPU < 75 % for 5 minutes" clears.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// A sample this far ahead of the evaluator's clock is refused.
pub const MAX_FUTURE_MS: i64 = 30_000;
/// Longest duration or window a rule may use.
pub const MAX_SPAN_MS: i64 = 24 * 3600 * 1000;
/// Samples a tracker keeps for window modes.
pub const MAX_WINDOW_SAMPLES: usize = 10_000;

/// An observed value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    /// A measurement (`85.5`), in the unit the rule names.
    Num(f64),
    /// A state (`failing`, `stopped`).
    State(String),
}

/// How a number is compared with the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cmp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

impl Cmp {
    fn holds(self, v: f64, t: f64) -> bool {
        match self {
            Cmp::Gt => v > t,
            Cmp::Ge => v >= t,
            Cmp::Lt => v < t,
            Cmp::Le => v <= t,
            Cmp::Eq => (v - t).abs() < f64::EPSILON,
            Cmp::Ne => (v - t).abs() >= f64::EPSILON,
        }
    }

    /// `>`, `<`, … for descriptions.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Cmp::Gt => ">",
            Cmp::Ge => "≥",
            Cmp::Lt => "<",
            Cmp::Le => "≤",
            Cmp::Eq => "=",
            Cmp::Ne => "≠",
        }
    }
}

/// What one value must be for the condition to hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Condition {
    /// A measurement compared with a threshold.
    Threshold { cmp: Cmp, value: f64 },
    /// The state is one of these.
    StateIn { states: Vec<String> },
    /// The state is none of these (and known).
    StateNotIn { states: Vec<String> },
}

impl Condition {
    /// Whether `v` satisfies the condition; a value of the wrong kind never
    /// does.
    #[must_use]
    pub fn holds(&self, v: &Value) -> bool {
        match (self, v) {
            (Condition::Threshold { cmp, value }, Value::Num(n)) => cmp.holds(*n, *value),
            (Condition::StateIn { states }, Value::State(s)) => states.iter().any(|x| x == s),
            (Condition::StateNotIn { states }, Value::State(s)) => !states.iter().any(|x| x == s),
            _ => false,
        }
    }

    fn validate(&self) -> Result<(), String> {
        match self {
            Condition::Threshold { value, .. } if !value.is_finite() => {
                Err("the threshold must be a finite number".into())
            }
            Condition::StateIn { states } | Condition::StateNotIn { states }
                if states.is_empty() || states.len() > 32 || states.iter().any(|s| s.is_empty() || s.len() > 64) =>
            {
                Err("list 1 to 32 states of 1 to 64 characters".into())
            }
            _ => Ok(()),
        }
    }

    /// `> 85`, `in failing, stopped`.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Condition::Threshold { cmp, value } => format!("{} {value}", cmp.symbol()),
            Condition::StateIn { states } => format!("is {}", states.join(" or ")),
            Condition::StateNotIn { states } => format!("is not {}", states.join(" or ")),
        }
    }
}

/// When the trigger counts as met (issue #11 rule semantics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Mode {
    /// The latest valid sample meets the condition (state changes, events).
    Immediate,
    /// Every valid sample for `for_ms`, without a gap in the evidence.
    Sustained { for_ms: i64 },
    /// The time-weighted average over the last `window_ms` meets the
    /// condition, with at least `min_coverage_pct` of the window covered by
    /// valid evidence. Numbers only.
    WindowAverage { window_ms: i64, min_coverage_pct: u8 },
    /// At least `count` distinct breaches started in the last `window_ms`.
    /// A breach starts when the condition becomes true after being false; a
    /// start less than `min_spacing_ms` after the previous one does not
    /// count, so faster sampling cannot inflate the count.
    Repeated { count: u32, window_ms: i64, min_spacing_ms: i64 },
    /// Any valid sample in the last `window_ms` met the condition (a single
    /// spike). It clears once no such sample is left in the window.
    AnyInWindow { window_ms: i64 },
}

/// When a firing condition is over.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Recovery {
    /// What every valid sample must meet to recover. `None`: the trigger is
    /// no longer met (for window modes: the window no longer qualifies, with
    /// enough coverage).
    #[serde(default)]
    pub condition: Option<Condition>,
    /// How long recovery must hold before the alarm resolves.
    #[serde(default)]
    pub hold_ms: i64,
}

/// A rule's evaluable part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleSpec {
    pub trigger: Condition,
    pub mode: Mode,
    pub recovery: Recovery,
}

impl RuleSpec {
    /// Checks bounds: finite thresholds, spans within a day, sane counts.
    pub fn validate(&self) -> Result<(), String> {
        self.trigger.validate()?;
        if let Some(c) = &self.recovery.condition {
            c.validate()?;
        }
        let span = |name: &str, v: i64, min: i64| {
            if (min..=MAX_SPAN_MS).contains(&v) {
                Ok(())
            } else {
                Err(format!("{name} must be between {min} ms and 24 hours"))
            }
        };
        span("the recovery hold", self.recovery.hold_ms, 0)?;
        match &self.mode {
            Mode::Immediate => {}
            Mode::Sustained { for_ms } => span("the duration", *for_ms, 1_000)?,
            Mode::WindowAverage { window_ms, min_coverage_pct } => {
                span("the window", *window_ms, 1_000)?;
                if !(1..=100).contains(min_coverage_pct) {
                    return Err("the minimum coverage is 1 to 100 %".into());
                }
                if !matches!(self.trigger, Condition::Threshold { .. }) {
                    return Err("an average needs a numeric threshold".into());
                }
            }
            Mode::Repeated { count, window_ms, min_spacing_ms } => {
                span("the window", *window_ms, 1_000)?;
                span("the spacing", *min_spacing_ms, 0)?;
                if !(1..=1000).contains(count) {
                    return Err("the count is 1 to 1000".into());
                }
            }
            Mode::AnyInWindow { window_ms } => span("the window", *window_ms, 1_000)?,
        }
        Ok(())
    }

    /// `> 85 for 30 min`, for messages and the UI.
    #[must_use]
    pub fn describe(&self) -> String {
        let c = self.trigger.describe();
        let mins = |ms: i64| {
            if ms % 60_000 == 0 { format!("{} min", ms / 60_000) } else { format!("{} s", ms / 1000) }
        };
        match &self.mode {
            Mode::Immediate => c,
            Mode::Sustained { for_ms } => format!("{c} for {}", mins(*for_ms)),
            Mode::WindowAverage { window_ms, .. } => format!("average {c} over {}", mins(*window_ms)),
            Mode::Repeated { count, window_ms, .. } => format!("{c} at least {count} times in {}", mins(*window_ms)),
            Mode::AnyInWindow { window_ms } => format!("{c} at any time in {}", mins(*window_ms)),
        }
    }
}

/// One timestamped piece of evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// When it was observed (evaluator clock, ms).
    pub at: i64,
    /// Until when it counts as evidence; after that the series is unknown.
    pub valid_until: i64,
    /// `None`: explicitly unknown (stale, executor unreachable).
    pub value: Option<Value>,
}

/// Where a series stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    Normal,
    /// The trigger holds but not yet long enough (sustained mode).
    Pending {
        since: i64,
    },
    Firing {
        since: i64,
    },
}

/// What happened while evaluating.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Became pending (sustained mode).
    Pending { at: i64 },
    /// Pending ended without firing (the condition stopped holding or the
    /// evidence became unknown).
    PendingCancelled { at: i64, unknown: bool },
    /// Fired; `value` is the evidence that completed it (the average for
    /// a window).
    Fired { at: i64, value: Option<Value> },
    /// Recovery held long enough.
    Resolved { at: i64, value: Option<Value> },
}

/// Why a sample was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refused {
    /// At or before the last applied sample: a duplicate or late arrival.
    Late,
    /// More than [`MAX_FUTURE_MS`] ahead of the evaluator's clock.
    Future,
    /// `valid_until` before `at`.
    Invalid,
}

/// One series' evaluation state. Serializable: persist it between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tracker {
    pub phase: Phase,
    /// The last applied sample time (checkpoint; later samples only).
    last_at: Option<i64>,
    /// The latest sample.
    latest: Option<Sample>,
    /// Start of the current unbroken run of valid samples meeting the
    /// trigger.
    breach_since: Option<i64>,
    /// Start of the current unbroken run of valid recovery evidence.
    clear_since: Option<i64>,
    /// Samples kept for window modes (bounded).
    window: VecDeque<Sample>,
    /// Starts of distinct breaches (repeated mode).
    breach_starts: VecDeque<i64>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            phase: Phase::Normal,
            last_at: None,
            latest: None,
            breach_since: None,
            clear_since: None,
            window: VecDeque::new(),
            breach_starts: VecDeque::new(),
        }
    }
}

impl Tracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether there is valid evidence at `now`.
    #[must_use]
    pub fn known(&self, now: i64) -> bool {
        self.latest.as_ref().is_some_and(|s| s.value.is_some() && now < s.valid_until)
    }

    /// The latest value, if still valid at `now`.
    #[must_use]
    pub fn current(&self, now: i64) -> Option<&Value> {
        self.latest.as_ref().filter(|s| now < s.valid_until).and_then(|s| s.value.as_ref())
    }

    /// Applies one sample (in time order) and evaluates at `now`.
    pub fn observe(&mut self, spec: &RuleSpec, s: Sample, now: i64) -> Result<Vec<Event>, Refused> {
        if s.valid_until < s.at {
            return Err(Refused::Invalid);
        }
        if s.at > now + MAX_FUTURE_MS {
            return Err(Refused::Future);
        }
        if self.last_at.is_some_and(|l| s.at <= l) {
            return Err(Refused::Late);
        }
        // A gap between the previous sample's validity and this one breaks
        // every run: nothing is known about that time.
        let gap = self.latest.as_ref().is_some_and(|p| s.at > p.valid_until);
        if gap {
            self.breach_since = None;
            self.clear_since = None;
        }
        let meets = s.value.as_ref().is_some_and(|v| spec.trigger.holds(v));
        let recovers = s.value.as_ref().is_some_and(|v| self.recovery_holds(spec, v));
        match &s.value {
            None => {
                self.breach_since = None;
                self.clear_since = None;
            }
            Some(_) => {
                if meets {
                    if self.breach_since.is_none() {
                        self.note_breach_start(spec, s.at);
                    }
                    self.breach_since.get_or_insert(s.at);
                } else {
                    self.breach_since = None;
                }
                if recovers {
                    self.clear_since.get_or_insert(s.at);
                } else {
                    self.clear_since = None;
                }
            }
        }
        self.last_at = Some(s.at);
        if matches!(spec.mode, Mode::WindowAverage { .. } | Mode::AnyInWindow { .. }) {
            self.window.push_back(s.clone());
            while self.window.len() > MAX_WINDOW_SAMPLES {
                self.window.pop_front();
            }
        }
        self.latest = Some(s);
        Ok(self.tick(spec, now))
    }

    fn note_breach_start(&mut self, spec: &RuleSpec, at: i64) {
        if let Mode::Repeated { min_spacing_ms, .. } = spec.mode {
            if self.breach_starts.back().is_none_or(|last| at - last >= min_spacing_ms) {
                self.breach_starts.push_back(at);
            }
            while self.breach_starts.len() > MAX_WINDOW_SAMPLES {
                self.breach_starts.pop_front();
            }
        }
    }

    /// Recovery evidence from one value: the recovery condition, or the
    /// trigger not met.
    fn recovery_holds(&self, spec: &RuleSpec, v: &Value) -> bool {
        match &spec.recovery.condition {
            Some(c) => c.holds(v),
            None => !spec.trigger.holds(v),
        }
    }

    /// Evaluates at `now` without a new sample (call on a timer): ends runs
    /// whose evidence ran out and fires or resolves when due.
    pub fn tick(&mut self, spec: &RuleSpec, now: i64) -> Vec<Event> {
        let mut out = Vec::new();
        let known = self.known(now);
        if !known {
            // Unknown: no run continues past the evidence.
            self.breach_since = None;
            self.clear_since = None;
        }
        self.trim_window(spec, now);
        match self.phase {
            Phase::Normal | Phase::Pending { .. } => {
                let (met, value) = self.trigger_met(spec, now);
                if met {
                    self.phase = Phase::Firing { since: now };
                    self.clear_since = None;
                    out.push(Event::Fired { at: now, value });
                } else if let (Mode::Sustained { .. }, Some(since)) = (&spec.mode, self.breach_since) {
                    if self.phase == Phase::Normal {
                        self.phase = Phase::Pending { since };
                        out.push(Event::Pending { at: since });
                    }
                } else if let Phase::Pending { .. } = self.phase {
                    self.phase = Phase::Normal;
                    out.push(Event::PendingCancelled { at: now, unknown: !known });
                }
            }
            Phase::Firing { .. } => {
                if self.recovered(spec, now) {
                    self.phase = Phase::Normal;
                    self.breach_since = None;
                    self.breach_starts.clear();
                    out.push(Event::Resolved { at: now, value: self.current(now).cloned() });
                }
            }
        }
        out
    }

    fn trim_window(&mut self, spec: &RuleSpec, now: i64) {
        let w = match spec.mode {
            Mode::WindowAverage { window_ms, .. } | Mode::AnyInWindow { window_ms } => window_ms,
            Mode::Repeated { window_ms, .. } => {
                while self.breach_starts.front().is_some_and(|t| *t < now - window_ms) {
                    self.breach_starts.pop_front();
                }
                return;
            }
            _ => return,
        };
        // Keep one sample that started before the window: it may cover its
        // beginning.
        while self.window.len() > 1 && self.window.get(1).is_some_and(|s| s.at <= now - w) {
            self.window.pop_front();
        }
    }

    /// Whether the trigger is met at `now`, and the evidence value.
    fn trigger_met(&self, spec: &RuleSpec, now: i64) -> (bool, Option<Value>) {
        match &spec.mode {
            Mode::Immediate => {
                let v = self.current(now).cloned();
                (v.as_ref().is_some_and(|v| spec.trigger.holds(v)), v)
            }
            Mode::Sustained { for_ms } => {
                let met = self.known(now) && self.breach_since.is_some_and(|t| now - t >= *for_ms);
                (met, self.current(now).cloned())
            }
            Mode::WindowAverage { window_ms, min_coverage_pct } => match self.average(now, *window_ms) {
                Some((avg, coverage)) if coverage * 100.0 >= f64::from(*min_coverage_pct) => {
                    (spec.trigger.holds(&Value::Num(avg)), Some(Value::Num(avg)))
                }
                _ => (false, None),
            },
            Mode::Repeated { count, .. } => {
                let n = self.breach_starts.len();
                (u32::try_from(n).unwrap_or(u32::MAX) >= *count, self.current(now).cloned())
            }
            Mode::AnyInWindow { window_ms } => {
                let hit = self
                    .window
                    .iter()
                    .rev()
                    .filter(|s| s.at > now - window_ms)
                    .find(|s| s.value.as_ref().is_some_and(|v| spec.trigger.holds(v)));
                (hit.is_some(), hit.and_then(|s| s.value.clone()))
            }
        }
    }

    /// Whether a firing condition has recovered at `now`. Never on unknown
    /// evidence.
    fn recovered(&self, spec: &RuleSpec, now: i64) -> bool {
        if !self.known(now) {
            return false;
        }
        let hold = spec.recovery.hold_ms;
        match (&spec.mode, &spec.recovery.condition) {
            // Window modes without their own recovery: the window no longer
            // qualifies, held for the hold time with valid evidence.
            (Mode::WindowAverage { window_ms, min_coverage_pct }, None) => match self.average(now, *window_ms) {
                Some((avg, cov)) if cov * 100.0 >= f64::from(*min_coverage_pct) => {
                    !spec.trigger.holds(&Value::Num(avg)) && self.clear_since.is_some_and(|t| now - t >= hold)
                }
                _ => false,
            },
            (Mode::AnyInWindow { .. } | Mode::Repeated { .. }, None) => {
                !self.trigger_met(spec, now).0 && self.clear_since.is_some_and(|t| now - t >= hold)
            }
            _ => self.clear_since.is_some_and(|t| now - t >= hold),
        }
    }

    /// Time-weighted average of numeric evidence over `[now - window, now]`
    /// and the share of the window it covers. Each sample counts from its
    /// time until the next sample or the end of its validity.
    fn average(&self, now: i64, window: i64) -> Option<(f64, f64)> {
        let start = now - window;
        let mut weighted = 0.0;
        let mut covered = 0_i64;
        let samples: Vec<&Sample> = self.window.iter().collect();
        for (i, s) in samples.iter().enumerate() {
            let Some(Value::Num(v)) = s.value else { continue };
            let next = samples.get(i + 1).map_or(now, |n| n.at);
            let from = s.at.max(start);
            let to = next.min(s.valid_until).min(now);
            if to > from {
                #[allow(clippy::cast_precision_loss)]
                {
                    weighted += v * (to - from) as f64;
                }
                covered += to - from;
            }
        }
        if covered == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        Some((weighted / covered as f64, covered as f64 / window as f64))
    }
}

#[cfg(test)]
mod tests;
