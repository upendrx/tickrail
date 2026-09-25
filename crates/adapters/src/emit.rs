//! Batches normalised messages from one venue frame and pushes them to the engine.

use crate::FeedStats;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tickrail_core::ring::Producer;
use tickrail_core::*;

/// Collects the messages produced from one frame. On [`Emitter::flush`] the last
/// one gets `LAST_IN_BATCH`, which is what tells the strategy the book is
/// consistent again. A venue frame that updates 40 levels must not trigger 40
/// strategy decisions on a half-applied book.
pub struct Emitter {
    inst: Instrument,
    seq: u64,
    tx: Producer<MdMsg>,
    stats: Arc<FeedStats>,
    pending: Vec<MdMsg>,
    /// Local receive time of the frame being parsed.
    pub recv_ts: u64,
}

impl Emitter {
    pub fn new(inst: Instrument, tx: Producer<MdMsg>, stats: Arc<FeedStats>) -> Self {
        Emitter { inst, seq: 0, tx, stats, pending: Vec::with_capacity(128), recv_ts: 0 }
    }

    pub fn instrument(&self) -> &Instrument {
        &self.inst
    }

    /// Exact decimal string to ticks.
    #[inline]
    pub fn px(&self, s: &str) -> Option<Price> {
        parse_fixed(s.as_bytes(), self.inst.price_decimals).map(Price)
    }

    /// Exact decimal string to lots.
    #[inline]
    pub fn qty(&self, s: &str) -> Option<Qty> {
        parse_fixed(s.as_bytes(), self.inst.qty_decimals).map(Qty)
    }

    pub fn push(&mut self, kind: MdKind, flags: u8, exch_ts: u64) {
        self.pending.push(MdMsg { seq: 0, exch_ts, recv_ts: self.recv_ts, instrument: self.inst.id, flags, kind });
    }

    pub fn level(&mut self, side: Side, price: Price, qty: Qty, exch_ts: u64) {
        self.push(MdKind::Level { side, price, qty }, 0, exch_ts);
    }

    pub fn trade(&mut self, aggressor: Side, price: Price, qty: Qty, exch_ts: u64) {
        self.push(MdKind::Trade { aggressor, price, qty }, 0, exch_ts);
        self.stats.venue_to_local_ns.store(Clock::wall_ns().saturating_sub(exch_ts), Ordering::Relaxed);
    }

    pub fn top(&mut self, side: Side, price: Price, qty: Qty, exch_ts: u64) {
        self.push(MdKind::Top { side, price, qty }, 0, exch_ts);
    }

    /// Starts a full snapshot: the engine drops its book first.
    pub fn clear(&mut self, exch_ts: u64) {
        self.push(MdKind::Clear, md_flags::SNAPSHOT, exch_ts);
    }

    /// Pushes everything collected so far as one consistent batch.
    pub fn flush(&mut self) {
        let Some(last) = self.pending.len().checked_sub(1) else { return };
        self.pending[last].flags |= md_flags::LAST_IN_BATCH;
        for mut m in self.pending.drain(..) {
            self.seq += 1;
            m.seq = self.seq;
            // A feed never blocks on a slow engine. Drop and count; the engine
            // sees the sequence gap.
            if self.tx.push(m).is_err() {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.stats.messages.fetch_add(1, Ordering::Relaxed);
    }

    /// Throws away a partially parsed frame.
    pub fn discard(&mut self) {
        self.pending.clear();
    }
}
