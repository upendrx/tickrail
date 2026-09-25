//! What a polling thread does when there is no work.
//!
//! Busy-spinning gives the lowest latency but burns a whole core, which is right
//! on a dedicated trading server and wrong on a shared laptop, a VM or a
//! Raspberry Pi. The strategy is picked per deployment in the config file.

use std::str::FromStr;
use std::time::Duration;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum WaitStrategy {
    /// Never give up the core. Lowest latency, 100% CPU per thread.
    Spin,
    /// `sched_yield` between polls. Lets other threads run, still ~100% CPU.
    Yield,
    /// Spin briefly, then yield, then sleep. Good default on shared machines.
    #[default]
    Backoff,
    /// Always sleep for the given duration between empty polls.
    Sleep(Duration),
}

impl WaitStrategy {
    /// Call on every empty poll; `idle` counts consecutive empty polls and is
    /// reset by the caller whenever work arrives.
    #[inline]
    pub fn idle(&self, idle: &mut u32) {
        *idle = idle.saturating_add(1);
        match *self {
            WaitStrategy::Spin => std::hint::spin_loop(),
            WaitStrategy::Yield => std::thread::yield_now(),
            WaitStrategy::Sleep(d) => std::thread::sleep(d),
            WaitStrategy::Backoff => {
                if *idle < 2_000 {
                    std::hint::spin_loop();
                } else if *idle < 2_100 {
                    std::thread::yield_now();
                } else {
                    std::thread::sleep(Duration::from_micros(50));
                }
            }
        }
    }
}

impl FromStr for WaitStrategy {
    type Err = String;

    /// `spin`, `yield`, `backoff`, or `sleep:<n>us` / `sleep:<n>ms`.
    fn from_str(s: &str) -> Result<Self, String> {
        match s.trim() {
            "spin" => Ok(WaitStrategy::Spin),
            "yield" => Ok(WaitStrategy::Yield),
            "backoff" => Ok(WaitStrategy::Backoff),
            other => {
                let d = other.strip_prefix("sleep:").ok_or_else(|| format!("unknown wait strategy `{other}`"))?;
                let (num, mul) = if let Some(n) = d.strip_suffix("us") {
                    (n, 1)
                } else if let Some(n) = d.strip_suffix("ms") {
                    (n, 1_000)
                } else {
                    return Err(format!("sleep duration needs a unit (us or ms): `{d}`"));
                };
                let n: u64 = num.parse().map_err(|_| format!("bad sleep duration `{d}`"))?;
                Ok(WaitStrategy::Sleep(Duration::from_micros(n * mul)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!("spin".parse(), Ok(WaitStrategy::Spin));
        assert_eq!("sleep:200us".parse(), Ok(WaitStrategy::Sleep(Duration::from_micros(200))));
        assert_eq!("sleep:2ms".parse(), Ok(WaitStrategy::Sleep(Duration::from_millis(2))));
        assert!("sleep:5".parse::<WaitStrategy>().is_err());
        assert!("fast".parse::<WaitStrategy>().is_err());
    }
}
