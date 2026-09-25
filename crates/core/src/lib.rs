//! Core building blocks shared by every component of the platform.
//!
//! Everything on the hot path is `Copy`, fixed-size and allocation-free:
//! prices and quantities are integers (ticks / lots), events are plain enums,
//! and threads talk only through single-producer/single-consumer rings.

pub mod clock;
pub mod cpu;
pub mod events;
pub mod ring;
pub mod rng;
pub mod types;
pub mod wait;

pub use clock::Clock;
pub use events::*;
pub use types::*;
pub use wait::WaitStrategy;

/// Pads and aligns a value to its own cache line(s) so two hot atomics written by
/// different cores never share a line (false sharing).
///
/// 128 bytes: Apple M-series report a 128-byte line (`hw.cachelinesize`) and
/// modern x86 prefetches adjacent 64-byte pairs, so 128 is the safe value on both.
#[derive(Default, Debug)]
#[repr(align(128))]
pub struct CachePadded<T>(pub T);

impl<T> core::ops::Deref for CachePadded<T> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        &self.0
    }
}
