//! Bounded lock-free single-producer / single-consumer ring buffer.
//!
//! This is the only way threads talk to each other on the hot path (the LMAX
//! Disruptor / Aeron IPC idea reduced to its core):
//! * no locks, no syscalls, no allocation after construction;
//! * producer and consumer indices live on separate cache lines;
//! * each side caches the other side's index, so the shared line is only
//!   touched when the ring *looks* full or empty: typically one cache miss per batch.

use crate::CachePadded;
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Shared<T> {
    /// Next slot the consumer will read. Written only by the consumer.
    head: CachePadded<AtomicUsize>,
    /// Next slot the producer will write. Written only by the producer.
    tail: CachePadded<AtomicUsize>,
    mask: usize,
    buf: Box<[UnsafeCell<MaybeUninit<T>>]>,
}

// SAFETY: slots are handed over with release/acquire on head/tail, and each slot
// is accessed by exactly one side at a time.
unsafe impl<T: Send> Sync for Shared<T> {}

pub struct Producer<T> {
    shared: Arc<Shared<T>>,
    tail: usize,
    cached_head: usize,
}

pub struct Consumer<T> {
    shared: Arc<Shared<T>>,
    head: usize,
    cached_tail: usize,
}

unsafe impl<T: Send> Send for Producer<T> {}
unsafe impl<T: Send> Send for Consumer<T> {}

/// Creates a ring holding `capacity` elements (rounded up to a power of two).
pub fn channel<T: Copy + Send>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let cap = capacity.max(2).next_power_of_two();
    let buf = (0..cap).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect();
    let shared = Arc::new(Shared {
        head: CachePadded(AtomicUsize::new(0)),
        tail: CachePadded(AtomicUsize::new(0)),
        mask: cap - 1,
        buf,
    });
    (Producer { shared: shared.clone(), tail: 0, cached_head: 0 }, Consumer { shared, head: 0, cached_tail: 0 })
}

impl<T: Copy + Send> Producer<T> {
    /// Non-blocking push. Returns the value back if the ring is full.
    #[inline]
    pub fn push(&mut self, value: T) -> Result<(), T> {
        let cap = self.shared.mask + 1;
        if self.tail.wrapping_sub(self.cached_head) == cap {
            self.cached_head = self.shared.head.load(Ordering::Acquire);
            if self.tail.wrapping_sub(self.cached_head) == cap {
                return Err(value);
            }
        }
        let slot = &self.shared.buf[self.tail & self.shared.mask];
        // SAFETY: the slot is not visible to the consumer until `tail` is published.
        unsafe { (*slot.get()).write(value) };
        self.tail = self.tail.wrapping_add(1);
        self.shared.tail.store(self.tail, Ordering::Release);
        Ok(())
    }

    /// Busy-spins until there is room. Use only where back-pressure is correct.
    #[inline]
    pub fn push_spin(&mut self, mut value: T) {
        loop {
            match self.push(value) {
                Ok(()) => return,
                Err(v) => {
                    value = v;
                    std::hint::spin_loop();
                }
            }
        }
    }

    pub fn capacity(&self) -> usize {
        self.shared.mask + 1
    }
}

impl<T: Copy + Send> Consumer<T> {
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.head == self.cached_tail {
            self.cached_tail = self.shared.tail.load(Ordering::Acquire);
            if self.head == self.cached_tail {
                return None;
            }
        }
        let slot = &self.shared.buf[self.head & self.shared.mask];
        // SAFETY: producer published this slot with a release store on `tail`.
        let value = unsafe { (*slot.get()).assume_init_read() };
        self.head = self.head.wrapping_add(1);
        self.shared.head.store(self.head, Ordering::Release);
        Some(value)
    }

    /// Approximate number of queued elements.
    pub fn len(&self) -> usize {
        self.shared.tail.load(Ordering::Acquire).wrapping_sub(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_and_full() {
        let (mut tx, mut rx) = channel::<u64>(4);
        for i in 0..4 {
            tx.push(i).unwrap();
        }
        assert_eq!(tx.push(99), Err(99));
        for i in 0..4 {
            assert_eq!(rx.pop(), Some(i));
        }
        assert_eq!(rx.pop(), None);
    }

    #[test]
    fn cross_thread_order_preserved() {
        const N: u64 = 2_000_000;
        let (mut tx, mut rx) = channel::<u64>(1024);
        let t = std::thread::spawn(move || {
            for i in 0..N {
                tx.push_spin(i);
            }
        });
        let mut expected = 0;
        while expected < N {
            if let Some(v) = rx.pop() {
                assert_eq!(v, expected);
                expected += 1;
            }
        }
        t.join().unwrap();
    }
}
