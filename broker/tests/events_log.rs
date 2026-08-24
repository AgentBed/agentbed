//! L01-AC06: durable `agentbed://events` append log and cursor replay.

use agentbed_broker::events::{EventCursor, EventLog, EventRecord};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

fn scratch() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agb4-events-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

#[test]
fn cursor_replay_is_strictly_after_acknowledged_event() {
    let dir = scratch();
    let log = EventLog::open(&dir).expect("open");
    let ids: Vec<_> = (0..5)
        .map(|i| {
            log.append(EventRecord {
                kind: "tx.state".to_owned(),
                payload: format!("{{\"n\":{i}}}"),
            })
            .expect("append")
        })
        .collect();

    let cursor = EventCursor::after(&ids[1]);
    let replay = log.replay(&cursor).expect("replay");
    assert_eq!(replay.len(), 3);
    assert_eq!(replay[0].seq, ids[2].seq);
    assert_eq!(replay.last().map(|e| e.seq), ids.last().map(|e| e.seq));
}

#[test]
fn cursor_survives_restart_without_loss_or_duplication() {
    let dir = scratch();
    let cursor = {
        let log = EventLog::open(&dir).expect("open");
        let _ = log.append(EventRecord {
            kind: "boot".to_owned(),
            payload: "{}".to_owned(),
        });
        let second = log
            .append(EventRecord {
                kind: "boot".to_owned(),
                payload: "{}".to_owned(),
            })
            .expect("append");
        EventCursor::after(&second)
    };

    {
        let log = EventLog::open(&dir).expect("reopen");
        let _ = log.append(EventRecord {
            kind: "tx.state".to_owned(),
            payload: "{}".to_owned(),
        });
        let _ = log.append(EventRecord {
            kind: "tx.state".to_owned(),
            payload: "{}".to_owned(),
        });
    }

    let log = EventLog::open(&dir).expect("final");
    let replay = log.replay(&cursor).expect("replay");
    assert_eq!(replay.len(), 2);
    let again = log.replay(&cursor).expect("replay again");
    assert_eq!(replay, again, "replay must be deterministic");
}

#[test]
fn malformed_and_beyond_tail_cursors_are_rejected() {
    let dir = scratch();
    let log = EventLog::open(&dir).expect("open");
    let first = log
        .append(EventRecord {
            kind: "ping".to_owned(),
            payload: "{}".to_owned(),
        })
        .expect("append");

    assert!(log.replay(&EventCursor::parse("not-valid")).is_err());
    assert!(log
        .replay(&EventCursor::foreign("other-log", first.seq))
        .is_err());

    let beyond = EventCursor::after_seq(log.log_id(), first.seq + 10_000);
    assert!(log.replay(&beyond).is_err());
}
