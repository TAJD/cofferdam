//! Per-check + per-phase timing collector for `cofferdam check --time-checks`
//! (CD-34). Purely additive: when no collector is passed into the engine,
//! zero timing code runs. Report goes to stderr only — never touches the
//! findings the caller returns, so JSON/robot output is unaffected.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Default)]
pub struct TimingCollector {
    checks: RefCell<HashMap<&'static str, Duration>>,
    phases: RefCell<Vec<(&'static str, Duration)>>,
}

impl TimingCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accumulate time spent in one check's `run`/`pass2`/`finalize` call,
    /// keyed by its stable check id. Called once per invocation, so a
    /// check that runs over many files ends up with a running total here.
    pub fn record_check(&self, check_id: &'static str, elapsed: Duration) {
        *self.checks.borrow_mut().entry(check_id).or_default() += elapsed;
    }

    /// Record one engine phase's wall-clock duration (discovery, run loop,
    /// pass 2, graph build, finalize A/B).
    pub fn record_phase(&self, phase: &'static str, elapsed: Duration) {
        self.phases.borrow_mut().push((phase, elapsed));
    }

    /// Render a descending-sorted table for stderr: phases in the order
    /// recorded, then per-check totals sorted by elapsed time.
    pub fn report(&self) -> String {
        let mut out = String::new();
        out.push_str("-- phase timing --\n");
        for (phase, dur) in self.phases.borrow().iter() {
            out.push_str(&format!(
                "  {phase:<24} {:>10.3}ms\n",
                dur.as_secs_f64() * 1000.0
            ));
        }
        out.push_str("-- per-check timing --\n");
        let mut checks: Vec<(&'static str, Duration)> =
            self.checks.borrow().iter().map(|(k, v)| (*k, *v)).collect();
        checks.sort_by_key(|c| std::cmp::Reverse(c.1));
        for (id, dur) in checks {
            out.push_str(&format!(
                "  {id:<40} {:>10.3}ms\n",
                dur.as_secs_f64() * 1000.0
            ));
        }
        out
    }
}
