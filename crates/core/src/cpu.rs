//! Thread placement.
//!
//! Production (Linux): pin each hot thread to an isolated core (`isolcpus`,
//! `nohz_full`) with `sched_setaffinity`. macOS has no hard affinity API on
//! Apple Silicon, so the best we can do is the `USER_INTERACTIVE` QoS class,
//! which makes the scheduler strongly prefer performance cores.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ThreadRole {
    /// Busy-spinning hot path (engine, venue sim, feed handler).
    LatencyCritical,
    /// Journaling, telemetry, web. Must never take a fast core away from the hot path.
    Background,
}

/// Applies placement for the calling thread. `core` is honoured on Linux only.
pub fn configure_current_thread(role: ThreadRole, core: Option<usize>) {
    #[cfg(target_os = "macos")]
    {
        let _ = core;
        const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
        const QOS_CLASS_UTILITY: u32 = 0x11;
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
        }
        let qos = match role {
            ThreadRole::LatencyCritical => QOS_CLASS_USER_INTERACTIVE,
            ThreadRole::Background => QOS_CLASS_UTILITY,
        };
        // SAFETY: affects only the calling thread.
        unsafe { pthread_set_qos_class_self_np(qos, 0) };
    }
    #[cfg(target_os = "linux")]
    {
        let _ = role;
        if let Some(core) = core {
            // SAFETY: cpu_set_t is plain data; we pass its exact size.
            unsafe {
                let mut set: libc::cpu_set_t = std::mem::zeroed();
                libc::CPU_SET(core, &mut set);
                libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
            }
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (role, core);
    }
}
