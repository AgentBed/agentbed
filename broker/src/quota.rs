//! Stage 5 accounting.
//!
//! # Admission is one atomic step, not check-then-charge
//!
//! `docs/effects.md` §1 makes quota a **mandatory final veto**: exhaustion
//! refuses even an approved or pre-authorized call. A ledger that answers "how
//! many have you used?" and separately accepts "charge one" cannot deliver
//! that, because the broker serves connections on concurrent threads: two
//! callers can both read `limit - 1`, both be allowed, and both execute. With
//! `calls_per_day: 1` that means two calls run against a budget of one.
//!
//! So the only operation exposed is [`QuotaLedger::try_admit`], which compares
//! against the limit and increments **while holding the same lock**. There is
//! no way to read the counter and act on it later, because that shape is the
//! bug.
//!
//! Two further properties:
//!
//! - The counter advances only when stages 1–4 have already allowed the call,
//!   so a refused call cannot consume an agent's budget — otherwise a hostile
//!   caller could exhaust another agent's budget with calls that were always
//!   going to fail.
//! - A poisoned lock refuses. An accounting failure must not read as "quota
//!   free".
//!
//! # What is still not durable
//!
//! The counters live in memory, so a restart clears them: a quota a crash
//! resets is a quota an agent can reset by crashing the broker. Durable
//! accounting shares the transaction WAL (`docs/effects.md` §3) and lands with
//! it at Gate 1–2.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds in the UTC day used for the rollover boundary.
const SECONDS_PER_DAY: u64 = 86_400;

/// One agent's usage within a day window.
#[derive(Debug, Clone, Copy)]
struct Window {
    /// Days since the Unix epoch, UTC.
    day: u64,
    /// Calls admitted within that day.
    used: u64,
}

/// Per-agent admission counters.
#[derive(Debug, Default)]
pub struct QuotaLedger {
    windows: Mutex<HashMap<String, Window>>,
}

impl QuotaLedger {
    /// Atomically admit one call for `agent_id` against `limit`.
    ///
    /// Returns `true` when the call is admitted and counted, `false` when the
    /// budget is exhausted (or accounting is unavailable). `None` for `limit`
    /// means the manifest declares no ceiling: the call is admitted and still
    /// counted, so the number is available for the ledger later.
    ///
    /// The check and the increment happen under one lock acquisition. Callers
    /// get an admission decision, never a snapshot to reason about.
    pub fn try_admit(&self, agent_id: &str, limit: Option<u64>) -> bool {
        let today = current_day();
        let Ok(mut windows) = self.windows.lock() else {
            // Fail closed: an accounting failure refuses rather than admits.
            return false;
        };

        let window = windows.entry(agent_id.to_owned()).or_insert(Window {
            day: today,
            used: 0,
        });

        // Roll over only forward. If the stored day is in the future — a clock
        // stepped backwards, or an NTP correction — keep counting in the old
        // window rather than handing out a fresh budget, which is the direction
        // that fails closed.
        if today > window.day {
            *window = Window {
                day: today,
                used: 0,
            };
        }

        if let Some(limit) = limit {
            if window.used >= limit {
                return false;
            }
        }
        window.used = window.used.saturating_add(1);
        true
    }

    /// Calls admitted for an agent in the current window. **Diagnostics only** —
    /// never use this to decide whether a call may proceed (see the module
    /// docs); [`Self::try_admit`] is the only admission path.
    #[must_use]
    pub fn admitted_today(&self, agent_id: &str) -> u64 {
        let today = current_day();
        self.windows.lock().map_or(u64::MAX, |windows| {
            windows
                .get(agent_id)
                .filter(|window| window.day >= today)
                .map_or(0, |window| window.used)
        })
    }
}

/// Days since the Unix epoch, UTC. A clock before the epoch reads as day 0.
fn current_day() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() / SECONDS_PER_DAY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    #[test]
    fn admits_up_to_the_limit_and_counts_per_agent() {
        let ledger = QuotaLedger::default();
        assert!(ledger.try_admit("a", Some(2)));
        assert!(ledger.try_admit("a", Some(2)));
        assert!(
            !ledger.try_admit("a", Some(2)),
            "the third call exceeds a limit of two"
        );

        // A different agent has its own budget.
        assert!(ledger.try_admit("b", Some(2)));
        assert_eq!(ledger.admitted_today("a"), 2);
        assert_eq!(ledger.admitted_today("b"), 1);
    }

    #[test]
    fn a_zero_limit_admits_nothing() {
        let ledger = QuotaLedger::default();
        assert!(!ledger.try_admit("a", Some(0)));
        assert_eq!(ledger.admitted_today("a"), 0);
    }

    #[test]
    fn an_absent_limit_admits_but_still_counts() {
        let ledger = QuotaLedger::default();
        assert!(ledger.try_admit("a", None));
        assert!(ledger.try_admit("a", None));
        assert_eq!(ledger.admitted_today("a"), 2);
    }

    #[test]
    fn concurrent_callers_cannot_exceed_the_limit() {
        // The regression test for check-then-charge: without atomic admission,
        // threads released together at the boundary all read the same count and
        // all proceed. Every thread starts from a barrier so they contend on
        // purpose rather than by luck.
        const THREADS: usize = 32;
        const LIMIT: u64 = 5;

        let ledger = Arc::new(QuotaLedger::default());
        let barrier = Arc::new(Barrier::new(THREADS));
        let admitted = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let ledger = Arc::clone(&ledger);
                let barrier = Arc::clone(&barrier);
                let admitted = Arc::clone(&admitted);
                std::thread::spawn(move || {
                    barrier.wait();
                    if ledger.try_admit("contended", Some(LIMIT)) {
                        admitted.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread joins");
        }

        assert_eq!(
            admitted.load(Ordering::SeqCst),
            usize::try_from(LIMIT).expect("limit fits"),
            "exactly the configured number of calls may be admitted"
        );
        assert_eq!(ledger.admitted_today("contended"), LIMIT);
    }

    #[test]
    fn a_poisoned_ledger_refuses_rather_than_admits() {
        let ledger = Arc::new(QuotaLedger::default());
        {
            let ledger = Arc::clone(&ledger);
            let _ = std::thread::spawn(move || {
                let _guard = ledger.windows.lock().expect("lock");
                panic!("poison the accounting lock");
            })
            .join();
        }
        assert!(
            !ledger.try_admit("a", Some(100)),
            "accounting failure must fail closed"
        );
        assert_eq!(
            ledger.admitted_today("a"),
            u64::MAX,
            "and must not read as unused"
        );
    }

    #[test]
    fn a_stale_window_rolls_over_but_a_backwards_clock_does_not() {
        let ledger = QuotaLedger::default();
        assert!(ledger.try_admit("a", Some(1)));
        assert!(!ledger.try_admit("a", Some(1)));

        // Yesterday's window rolls over: today's budget is fresh.
        if let Ok(mut windows) = ledger.windows.lock() {
            if let Some(window) = windows.get_mut("a") {
                window.day = current_day().saturating_sub(1);
            }
        }
        assert!(
            ledger.try_admit("a", Some(1)),
            "a new day grants a new budget"
        );

        // A window dated in the future (clock stepped back) must NOT reset.
        if let Ok(mut windows) = ledger.windows.lock() {
            if let Some(window) = windows.get_mut("a") {
                window.day = current_day().saturating_add(1);
                window.used = 1;
            }
        }
        assert!(
            !ledger.try_admit("a", Some(1)),
            "a backwards clock must not refill the budget"
        );
    }
}
