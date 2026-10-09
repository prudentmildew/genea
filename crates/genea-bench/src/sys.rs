//! Kernel clocks and per-process accounting.
//!
//! Times are `mach_absolute_time` in nanoseconds since boot: the clock
//! Genea's journal uses, so harness and journal timestamps compare directly.
//! `proc_pid_rusage` works on any process of the same user, without
//! entitlements.

#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

fn ticks_to_ns(ticks: u64) -> u64 {
    let mut timebase = Timebase { numer: 0, denom: 0 };
    // SAFETY: writes into the struct we pass.
    unsafe { mach_timebase_info(&mut timebase) };
    (ticks as u128 * timebase.numer as u128 / timebase.denom.max(1) as u128) as u64
}

/// Now, in ns since boot.
pub fn now_ns() -> u64 {
    // SAFETY: no preconditions.
    ticks_to_ns(unsafe { mach_absolute_time() })
}

/// A process's kernel accounting at one moment.
#[derive(Clone, Copy, Debug)]
pub struct Usage {
    /// User + system CPU time.
    pub cpu_ns: u64,
    /// Package idle and interrupt wake-ups.
    pub wakeups: u64,
    /// `phys_footprint`, what Activity Monitor calls Memory.
    pub footprint_bytes: u64,
    /// When the kernel started the process, in ns since boot.
    pub start_ns: u64,
}

impl Usage {
    pub fn footprint_mb(&self) -> f64 {
        self.footprint_bytes as f64 / 1024.0 / 1024.0
    }
}

/// `None` once the process is gone.
pub fn usage(pid: u32) -> Option<Usage> {
    // SAFETY: proc_pid_rusage fills the struct we pass.
    unsafe {
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        let rc = libc::proc_pid_rusage(
            pid as i32,
            libc::RUSAGE_INFO_V4,
            &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
        );
        (rc == 0).then(|| Usage {
            cpu_ns: ticks_to_ns(info.ri_user_time + info.ri_system_time),
            wakeups: info.ri_pkg_idle_wkups + info.ri_interrupt_wkups,
            footprint_bytes: info.ri_phys_footprint,
            start_ns: ticks_to_ns(info.ri_proc_start_abstime),
        })
    }
}
