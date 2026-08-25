//! L03 RED — bounded spawned fixture exercising the production process-group fencer.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use agentbed_watchdogd::fencing::ProductionProcessGroupFencer;
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const FENCE_BOUNDED_WAIT: Duration = Duration::from_secs(2);
const FIXTURE_CLEANUP_BUDGET: Duration = Duration::from_secs(5);
const CLEANUP_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Child in its own process group (`process_group(0)`); bounded group cleanup on drop.
struct SpawnedFixture {
    survivor: Child,
    unrelated: Child,
    pgid: i32,
}

impl SpawnedFixture {
    fn start() -> Self {
        let survivor = Command::new("sh")
            .args(["-c", "trap '' TERM; while true; do sleep 1; done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("spawn term-resistant survivor");
        let pgid = survivor.id() as i32;
        let unrelated = Command::new("sleep")
            .arg("600")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn unrelated sleeper");
        Self {
            survivor,
            unrelated,
            pgid,
        }
    }

    fn survivor_pid(&self) -> i32 {
        self.survivor.id() as i32
    }

    fn unrelated_pid(&self) -> i32 {
        self.unrelated.id() as i32
    }
}

impl Drop for SpawnedFixture {
    fn drop(&mut self) {
        let deadline = Instant::now() + FIXTURE_CLEANUP_BUDGET;
        let survivor_pid = self.survivor_pid();
        let unrelated_pid = self.unrelated_pid();
        terminate_process_group(self.pgid, deadline);
        reap_child_bounded(&mut self.survivor, survivor_pid, deadline);
        reap_child_bounded(&mut self.unrelated, unrelated_pid, deadline);
        if process_alive(self.pgid) {
            let _ = Command::new("kill")
                .args(["-KILL", &format!("-{}", self.pgid)])
                .status();
        }
    }
}

fn poll_try_wait_until(child: &mut Child, deadline: Instant) -> bool {
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return true;
        }
        thread::sleep(CLEANUP_POLL_INTERVAL);
    }
    false
}

fn reap_child_bounded(child: &mut Child, pid: i32, deadline: Instant) {
    let _ = child.kill();
    if poll_try_wait_until(child, deadline) {
        return;
    }
    let _ = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status();
    let _ = poll_try_wait_until(child, deadline);
}

fn terminate_process_group(pgid: i32, deadline: Instant) {
    let _ = Command::new("kill")
        .args(["-TERM", &format!("-{pgid}")])
        .status();
    let term_deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < term_deadline.min(deadline) && process_alive(pgid) {
        thread::sleep(CLEANUP_POLL_INTERVAL);
    }
    let _ = Command::new("kill")
        .args(["-KILL", &format!("-{pgid}")])
        .status();
}

fn process_alive(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[test]
fn l03_ac08_spawned_fixture_term_survivor_kill_confirms_exit_without_harming_unrelated() {
    let fixture = SpawnedFixture::start();
    assert!(process_alive(fixture.survivor_pid()));
    assert!(process_alive(fixture.unrelated_pid()));
    let unrelated_pid = fixture.unrelated_pid();

    let fencer = ProductionProcessGroupFencer::new();
    fencer
        .fence_group(fixture.pgid, FENCE_BOUNDED_WAIT)
        .expect("fence group");

    assert!(!process_alive(fixture.survivor_pid()));
    assert!(!process_alive(fixture.pgid));
    assert!(process_alive(unrelated_pid));
}
