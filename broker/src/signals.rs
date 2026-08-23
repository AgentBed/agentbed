//! Termination handling.
//!
//! The broker must keep running until it is told to stop, and must then stop
//! *cleanly* — joining connection threads and removing its socket. Signals are
//! blocked in the main thread before any other thread exists, so every thread
//! the broker later spawns inherits the block and the disposition is decided in
//! exactly one place, by [`wait_for_termination`].

/// Block SIGTERM and SIGINT so they can be waited for rather than delivered.
///
/// Must be called **before** any thread is spawned: threads inherit the signal
/// mask at creation, and a connection thread that could take the signal would
/// terminate the process without the clean shutdown path.
pub fn block_termination_signals() -> Result<(), String> {
    // SAFETY: `sigset_t` is zero-initialized and then filled by the libc
    // helpers before use; `pthread_sigmask` only reads it.
    #[allow(unsafe_code)]
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        if libc::sigemptyset(std::ptr::addr_of_mut!(set)) != 0 {
            return Err("sigemptyset failed".to_owned());
        }
        for signal in [libc::SIGTERM, libc::SIGINT] {
            if libc::sigaddset(std::ptr::addr_of_mut!(set), signal) != 0 {
                return Err("sigaddset failed".to_owned());
            }
        }
        if libc::pthread_sigmask(
            libc::SIG_BLOCK,
            std::ptr::addr_of!(set),
            std::ptr::null_mut(),
        ) != 0
        {
            return Err("pthread_sigmask failed".to_owned());
        }
    }
    Ok(())
}

/// Wait until SIGTERM or SIGINT arrives.
///
/// Deliberately not "read stdin to EOF": a unit with `StandardInput=null`, or
/// any invocation redirecting from `/dev/null`, would see EOF immediately and
/// the broker would exit the moment it started.
pub fn wait_for_termination() {
    // SAFETY: same set as above; `sigwait` writes only to `received`.
    #[allow(unsafe_code)]
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(std::ptr::addr_of_mut!(set));
        libc::sigaddset(std::ptr::addr_of_mut!(set), libc::SIGTERM);
        libc::sigaddset(std::ptr::addr_of_mut!(set), libc::SIGINT);

        let mut received: libc::c_int = 0;
        loop {
            let rc = libc::sigwait(std::ptr::addr_of!(set), std::ptr::addr_of_mut!(received));
            if rc == 0 {
                return;
            }
            if rc != libc::EINTR {
                // Nothing sensible remains: park rather than spin, and let the
                // supervisor's SIGKILL end the process.
                loop {
                    std::thread::park();
                }
            }
        }
    }
}
