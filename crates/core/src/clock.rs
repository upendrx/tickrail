//! Monotonic nanosecond clock.
//!
//! On macOS this reads `mach_absolute_time` directly (a user-space counter read,
//! no syscall). On Apple Silicon that counter ticks at 24 MHz, so resolution is
//! about 41.7 ns and measured latencies land on multiples of it. Elsewhere it
//! uses `Instant` (`clock_gettime(CLOCK_MONOTONIC)` via the vDSO on Linux, a few
//! ns resolution). A TSC-based clock is a natural addition for x86 servers.

#[derive(Copy, Clone, Debug)]
pub struct Clock {
    numer: u64,
    denom: u64,
}

#[cfg(target_os = "macos")]
mod imp {
    #[repr(C)]
    pub struct Timebase {
        pub numer: u32,
        pub denom: u32,
    }
    unsafe extern "C" {
        pub fn mach_absolute_time() -> u64;
        pub fn mach_timebase_info(info: *mut Timebase) -> i32;
    }

    pub fn timebase() -> (u64, u64) {
        let mut tb = Timebase { numer: 0, denom: 0 };
        // SAFETY: plain out-parameter call.
        unsafe { mach_timebase_info(&mut tb) };
        (tb.numer as u64, tb.denom as u64)
    }

    #[inline(always)]
    pub fn raw() -> u64 {
        // SAFETY: no preconditions.
        unsafe { mach_absolute_time() }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();

    pub fn timebase() -> (u64, u64) {
        START.get_or_init(Instant::now);
        (1, 1)
    }

    #[inline(always)]
    pub fn raw() -> u64 {
        START.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }
}

impl Clock {
    pub fn new() -> Self {
        let (numer, denom) = imp::timebase();
        Clock { numer, denom }
    }

    /// Monotonic nanoseconds since boot (macOS) / process start (elsewhere).
    #[inline(always)]
    pub fn now(&self) -> u64 {
        let raw = imp::raw();
        if self.numer == self.denom { raw } else { ((raw as u128 * self.numer as u128) / self.denom as u128) as u64 }
    }

    /// Wall-clock nanoseconds since the Unix epoch (for logs, not for latency).
    pub fn wall_ns() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}
