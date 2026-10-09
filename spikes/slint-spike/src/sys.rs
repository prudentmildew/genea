//! PROTOTYPE. Kernel per-process accounting (CPU time, wake-ups, memory
//! footprint, process start) via `proc_pid_rusage`.

use std::time::{Duration, Instant};

pub struct Usage {
    pub cpu: Duration,
    pub wakeups: u64,
    pub footprint_mb: f64,
    /// Process start, in mach absolute time.
    pub start_abs: u64,
}

pub fn usage(pid: i32) -> Option<Usage> {
    unsafe {
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        let rc = libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V4,
            &mut info as *mut _ as *mut libc::rusage_info_t,
        );
        (rc == 0).then(|| Usage {
            cpu: abs_to_duration(info.ri_user_time + info.ri_system_time),
            wakeups: info.ri_pkg_idle_wkups + info.ri_interrupt_wkups,
            footprint_mb: info.ri_phys_footprint as f64 / 1024.0 / 1024.0,
            start_abs: info.ri_proc_start_abstime,
        })
    }
}

#[allow(deprecated)]
pub fn abs_to_duration(abs: u64) -> Duration {
    let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
    unsafe { libc::mach_timebase_info(&mut timebase) };
    Duration::from_nanos(abs * timebase.numer as u64 / timebase.denom as u64)
}

#[allow(deprecated)]
pub fn now_abs() -> u64 {
    unsafe { libc::mach_absolute_time() }
}

/// Time from this process's start (as the kernel recorded it) to `at`.
pub fn since_process_start(at: Instant) -> Duration {
    let (now, now_abs) = (Instant::now(), now_abs());
    let start_abs = usage(std::process::id() as i32).unwrap().start_abs;
    abs_to_duration(now_abs - start_abs) - now.duration_since(at)
}
