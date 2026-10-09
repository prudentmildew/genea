use std::time::{Duration, Instant, SystemTime};

/// Called once when a timer is due. It runs on a host-owned thread (the real
/// clock) or on the thread that advances a manual clock, never on the main
/// thread, so it should only post work back to the core.
pub type TimerCallback = Box<dyn FnOnce() + Send + 'static>;

/// The host clock.
///
/// The real clock is the monotonic system clock. The test clock only moves
/// when a test advances it, so timers and timeouts are deterministic.
pub trait Clock: Send + Sync {
    /// The current time on this clock.
    fn now(&self) -> Instant;

    /// Calls `fire` once `delay` has passed on this clock.
    fn after(&self, delay: Duration, fire: TimerCallback);

    /// Wall-clock time, only for facts that must outlive the process (when
    /// the update check last ran). Measure durations with [`now`](Self::now).
    fn system_time(&self) -> SystemTime;
}
