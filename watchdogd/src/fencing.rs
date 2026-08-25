//! Production process-group fencing with bounded termination and reap checks.

use crate::error::FenceError;
use crate::interfaces::{FenceStage, ProcessGroupFence, SignalKind};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const TERM_GRACE_MAX: Duration = Duration::from_millis(100);

#[derive(Debug, Default)]
pub struct ProductionProcessGroupFencer {
    last_pgid: Mutex<Option<i32>>,
}

impl ProductionProcessGroupFencer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fence_group(&self, process_group: i32, timeout: Duration) -> Result<(), FenceError> {
        if process_group <= 0 {
            return Err(FenceError::SignalFailed);
        }
        signal_group(process_group, libc::SIGTERM)?;
        wait_until_absent(process_group, timeout.min(TERM_GRACE_MAX));
        if group_alive(process_group) {
            signal_group(process_group, libc::SIGKILL)?;
            wait_until_absent(process_group, timeout);
        }
        reap_group_children(process_group);
        if group_alive(process_group) {
            return Err(FenceError::Incomplete);
        }
        Ok(())
    }
}

impl ProcessGroupFence for ProductionProcessGroupFencer {
    fn signal(&self, kind: SignalKind, pgid: i32) -> Result<(), FenceError> {
        *self
            .last_pgid
            .lock()
            .map_err(|_| FenceError::SignalFailed)? = Some(pgid);
        match kind {
            SignalKind::Term => signal_group(pgid, libc::SIGTERM),
            SignalKind::Kill => signal_group(pgid, libc::SIGKILL),
        }
    }

    fn group_alive(&self, _stage: FenceStage) -> bool {
        let pgid = self
            .last_pgid
            .lock()
            .ok()
            .and_then(|guard| *guard)
            .unwrap_or(0);
        if pgid <= 0 {
            return false;
        }
        group_alive(pgid)
    }

    fn bounded_wait(&self, timeout: Duration) -> Result<(), FenceError> {
        let pgid = self
            .last_pgid
            .lock()
            .ok()
            .and_then(|guard| *guard)
            .unwrap_or(0);
        if pgid <= 0 {
            return Err(FenceError::Incomplete);
        }
        wait_until_absent(pgid, timeout);
        if group_alive(pgid) {
            return Err(FenceError::Incomplete);
        }
        Ok(())
    }
}

fn wait_until_absent(process_group: i32, timeout: Duration) {
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    while Instant::now() < deadline {
        reap_group_children(process_group);
        if !group_alive(process_group) {
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    reap_group_children(process_group);
}

#[allow(unsafe_code)]
fn signal_group(process_group: i32, signal: i32) -> Result<(), FenceError> {
    let result = unsafe { libc::kill(process_group.saturating_neg(), signal) };
    if result == 0 {
        return Ok(());
    }
    if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(FenceError::SignalFailed)
}

#[allow(unsafe_code)]
fn group_alive(process_group: i32) -> bool {
    let result = unsafe { libc::kill(process_group.saturating_neg(), 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[allow(unsafe_code)]
fn reap_group_children(process_group: i32) {
    loop {
        let result = unsafe {
            libc::waitpid(
                process_group.saturating_neg(),
                std::ptr::null_mut(),
                libc::WNOHANG,
            )
        };
        if result <= 0 {
            break;
        }
    }
}
