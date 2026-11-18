//! Signal handlers only store atomics; process cleanup happens in the runner.
//! SIGKILL cannot be caught: killing tick that way can orphan its child group.
use std::{
    io,
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
};
pub(super) static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static SIGNAL: AtomicI32 = AtomicI32::new(0);
pub(super) fn number() -> i32 {
    SIGNAL.load(Ordering::Relaxed)
}
#[cfg(unix)]
extern "C" fn handle(signal: libc::c_int) {
    let _ = SIGNAL.compare_exchange(
        0,
        signal,
        Ordering::Relaxed,
        Ordering::Relaxed,
    );
    INTERRUPTED.store(true, Ordering::Release);
}
pub(super) struct Signals {
    #[cfg(unix)]
    previous: Vec<(i32, libc::sigaction)>,
}
impl Signals {
    pub fn install() -> io::Result<Self> {
        INTERRUPTED.store(false, Ordering::Relaxed);
        SIGNAL.store(0, Ordering::Relaxed);
        let mut guard = Self {
            #[cfg(unix)]
            previous: Vec::new(),
        };
        #[cfg(unix)]
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = handle as *const () as usize;
            action.sa_flags = libc::SA_RESTART;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
            }
            if unsafe { libc::sigaction(signal, &action, &mut old) } != 0 {
                return Err(io::Error::last_os_error());
            }
            guard.previous.push((signal, old));
        }
        Ok(guard)
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        #[cfg(unix)]
        for (signal, previous) in &self.previous {
            unsafe {
                libc::sigaction(*signal, previous, std::ptr::null_mut());
            }
        }
    }
}
