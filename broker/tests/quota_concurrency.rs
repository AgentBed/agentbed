//! Stage 5 under concurrency.
//!
//! `docs/effects.md` §1 calls quota a **mandatory final veto**. The broker
//! serves connections on concurrent threads, so that guarantee only holds if
//! admission is atomic: an earlier version read a `calls_used` snapshot and
//! incremented afterwards, which let two callers at the boundary both observe
//! `limit - 1`, both be allowed, and both execute.
//!
//! This is the regression test at the level where it matters — real
//! connections, real threads, the real dispatcher — rather than only at the
//! ledger's own API.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

mod support;

use agentbed_protocol::wire::{DecisionStage, ErrorCode};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Barrier;
use support::{
    read_response, request_body, send_frame, Harness, AGENT_TIGHT_QUOTA, TIGHT_QUOTA_LIMIT,
    TOKEN_A, TOKEN_TIGHT_QUOTA,
};

#[test]
fn concurrent_calls_cannot_exceed_the_daily_budget() {
    const CALLERS: usize = 16;

    let harness = Harness::start();
    let allowed = AtomicUsize::new(0);
    let quota_refusals = AtomicUsize::new(0);
    // Every caller waits here, so they contend at the boundary on purpose
    // rather than by scheduling luck.
    let barrier = Barrier::new(CALLERS);

    std::thread::scope(|scope| {
        for caller in 0..CALLERS {
            let (harness, allowed, quota_refusals, barrier) =
                (&harness, &allowed, &quota_refusals, &barrier);
            scope.spawn(move || {
                // Connect before the barrier: the socket handshake must not be
                // part of what the threads are racing on.
                let mut stream = harness.connect();
                barrier.wait();

                send_frame(
                    &mut stream,
                    &request_body(&format!("01J-q{caller}"), TOKEN_TIGHT_QUOTA),
                );
                let response = read_response(&mut stream).expect("every caller gets one response");

                match response.error {
                    None => {
                        assert!(
                            response.result.is_some(),
                            "an allowed call returns its result"
                        );
                        allowed.fetch_add(1, Ordering::SeqCst);
                    }
                    Some(error) => {
                        assert_eq!(error.code, ErrorCode::QuotaExhausted);
                        assert_eq!(error.stage, Some(DecisionStage::Quota));
                        assert!(
                            response.result.is_none(),
                            "a refused call returns no result"
                        );
                        quota_refusals.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
    });

    assert_eq!(
        allowed.load(Ordering::SeqCst),
        TIGHT_QUOTA_LIMIT,
        "exactly the declared budget may be served, however many callers race for it"
    );
    assert_eq!(
        quota_refusals.load(Ordering::SeqCst),
        CALLERS - TIGHT_QUOTA_LIMIT,
        "every other caller is refused by the quota veto, not dropped"
    );

    // The audit trail agrees with the wire: exactly the budget was authorized.
    let records = harness.wait_for_records(CALLERS);
    let authorized = records
        .iter()
        .filter(|r| r.allowed && r.agent_id.as_deref() == Some(AGENT_TIGHT_QUOTA))
        .count();
    assert_eq!(authorized, TIGHT_QUOTA_LIMIT);
}

#[test]
fn one_agents_exhausted_budget_does_not_affect_another() {
    let harness = Harness::start();

    // Spend the tight agent's whole budget.
    for i in 0..TIGHT_QUOTA_LIMIT {
        let mut stream = harness.connect();
        send_frame(
            &mut stream,
            &request_body(&format!("01J-spend{i}"), TOKEN_TIGHT_QUOTA),
        );
        assert!(read_response(&mut stream)
            .expect("a response")
            .error
            .is_none());
    }
    let mut stream = harness.connect();
    send_frame(&mut stream, &request_body("01J-over", TOKEN_TIGHT_QUOTA));
    assert_eq!(
        read_response(&mut stream)
            .expect("a response")
            .error
            .expect("a refusal")
            .code,
        ErrorCode::QuotaExhausted
    );

    // A different agent's budget is untouched: counters are per identity.
    let mut other = harness.connect();
    send_frame(&mut other, &request_body("01J-other", TOKEN_A));
    assert!(read_response(&mut other)
        .expect("a response")
        .error
        .is_none());
}
