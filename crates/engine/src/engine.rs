//! One thread, one event loop, no locks.
//!
//! Every market-data message takes the same path, synchronously:
//! book.apply, venue.on_market, strategy, risk, OMS, venue.send.
//! Keeping all of it on one thread (the "single writer" principle from LMAX)
//! means no locking, no cache-line ping-pong, and replays that behave exactly
//! like the live run.

use crate::stats::{
    BOOK_LEVELS, EngineStats, FILL_ROWS, FillRow, LatKind, LatSample, ORDER_ROWS, OrderRow, TRADE_ROWS, TradeRow,
};
use crate::venue::Venue;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tickrail_book::L2Book;
use tickrail_core::ring::{Consumer, Producer};
use tickrail_core::*;
use tickrail_journal::Record;
use tickrail_oms::{Oms, OrdStatus};
use tickrail_risk::RiskEngine;
use tickrail_strategies::{Ctx, Intent, Strategy};

const SNAPSHOT_EVERY_NS: u64 = 100_000_000;

#[derive(Default)]
pub struct Outputs {
    pub journal: Option<Producer<Record>>,
    pub latency: Option<Producer<LatSample>>,
    pub stats: Option<Producer<EngineStats>>,
}

/// Knobs that are not strategy or risk parameters.
#[derive(Clone, Debug)]
pub struct EngineOptions {
    /// Replay mode: event timestamps are historical, so latency is measured
    /// from processing start instead of the message's receive time.
    pub replay: bool,
    /// What the engine thread does when idle.
    pub wait: WaitStrategy,
    /// Core to pin the engine thread to (Linux only).
    pub core: Option<usize>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        EngineOptions { replay: false, wait: WaitStrategy::Spin, core: None }
    }
}

pub struct Engine<S: Strategy, V: Venue> {
    inst: Instrument,
    clock: Clock,
    pub book: L2Book,
    pub oms: Oms,
    pub risk: RiskEngine,
    pub strategy: S,
    pub venue: V,
    out: Outputs,
    intents: Vec<Intent>,
    execs: Vec<ExecMsg>,
    replay: bool,
    opts: EngineOptions,
    /// Event time (ns) of the message being processed.
    now: u64,
    last_seq: u64,
    last_snapshot: u64,
    kill_handled: bool,
    pub counters: EngineStats,
    /// Circular buffers for the dashboard (newest at `*_head - 1`).
    trades: [TradeRow; TRADE_ROWS],
    trades_head: usize,
    fills: [FillRow; FILL_ROWS],
    fills_head: usize,
}

impl<S: Strategy, V: Venue> Engine<S, V> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inst: Instrument,
        clock: Clock,
        strategy: S,
        venue: V,
        oms: Oms,
        risk: RiskEngine,
        out: Outputs,
        opts: EngineOptions,
    ) -> Self {
        Engine {
            inst,
            clock,
            book: L2Book::with_capacity(1024),
            oms,
            risk,
            strategy,
            venue,
            out,
            intents: Vec::with_capacity(64),
            execs: Vec::with_capacity(256),
            replay: opts.replay,
            opts,
            now: 0,
            last_seq: 0,
            last_snapshot: 0,
            kill_handled: false,
            counters: EngineStats::default(),
            trades: [TradeRow::default(); TRADE_ROWS],
            trades_head: 0,
            fills: [FillRow::default(); FILL_ROWS],
            fills_head: 0,
        }
    }

    #[inline(always)]
    fn journal(&mut self, r: Record) {
        if let Some(j) = self.out.journal.as_mut()
            && j.push(r).is_err()
        {
            self.counters.journal_dropped += 1;
        }
    }

    #[inline(always)]
    fn latency(&mut self, kind: LatKind, ns: u64) {
        if let Some(l) = self.out.latency.as_mut() {
            let _ = l.push(LatSample { kind, ns });
        }
    }

    /// Hot path entry point.
    #[inline]
    pub fn on_md(&mut self, md: &MdMsg) {
        let entry = self.clock.now();
        let origin = if self.replay {
            entry
        } else {
            self.latency(LatKind::MdQueue, entry.saturating_sub(md.recv_ts));
            md.recv_ts
        };
        if self.last_seq != 0 && md.seq > self.last_seq + 1 {
            self.counters.md_gaps += md.seq - self.last_seq - 1;
        }
        self.last_seq = md.seq;
        self.counters.md_msgs += 1;
        self.now = md.recv_ts;
        self.journal(Record::Md(*md));

        self.book.apply(&md.kind);
        if let MdKind::Trade { aggressor, price, qty } = md.kind {
            self.counters.trades_seen += 1;
            self.trades[self.trades_head % TRADE_ROWS] =
                TradeRow { ts_ns: md.exch_ts, price: price.0, qty: qty.0, side: aggressor as u8 };
            self.trades_head += 1;
            let mut ctx = Ctx { now: self.now, book: &self.book, oms: &self.oms, intents: &mut self.intents };
            self.strategy.on_trade(&mut ctx, price, qty, aggressor);
        }
        self.venue.on_market(md, &self.book);
        self.drain_venue(origin);

        if md.is_last_in_batch() && !self.book.is_crossed() && !self.risk.killed() {
            self.counters.book_batches += 1;
            let mut ctx = Ctx { now: self.now, book: &self.book, oms: &self.oms, intents: &mut self.intents };
            self.strategy.on_book(&mut ctx);
        }
        self.process_intents(origin);
        if !self.replay {
            self.latency(LatKind::MdProcess, self.clock.now().saturating_sub(entry));
        }
    }

    /// Pulls execution reports from the venue and applies them.
    #[inline]
    pub fn drain_venue(&mut self, origin: u64) {
        self.venue.poll(&mut self.execs);
        if self.execs.is_empty() {
            return;
        }
        let mut execs = std::mem::take(&mut self.execs);
        for e in execs.drain(..) {
            self.journal(Record::Exec(e));
            match e.kind {
                ExecKind::Ack if !self.replay => {
                    if let Some(o) = self.oms.order(e.cl_id) {
                        let sent = o.sent_ts;
                        self.latency(LatKind::OrderAck, self.clock.now().saturating_sub(sent));
                    }
                }
                ExecKind::Rejected { .. } => self.counters.venue_rejects += 1,
                _ => {}
            }
            if let Some(fill) = self.oms.on_exec(&e) {
                let mid_x2 = match (self.book.best_bid(), self.book.best_ask()) {
                    (Some(b), Some(a)) => b.price.0 + a.price.0,
                    _ => 0,
                };
                self.fills[self.fills_head % FILL_ROWS] = FillRow {
                    ts_ns: e.ts,
                    cl_id: fill.cl_id,
                    side: fill.side as u8,
                    price: fill.price.0,
                    qty: fill.qty.0,
                    maker: fill.liquidity == Liquidity::Maker,
                    mid_x2,
                };
                self.fills_head += 1;
                let mut ctx = Ctx { now: self.now, book: &self.book, oms: &self.oms, intents: &mut self.intents };
                self.strategy.on_fill(&mut ctx, &fill);
                if let Some(mid) = self.book.mid() {
                    self.risk.check_pnl(self.oms.pnl(mid));
                }
            }
        }
        self.execs = execs;
        self.process_intents(origin);
    }

    /// Risk-checks strategy intents and sends the survivors.
    #[inline]
    fn process_intents(&mut self, origin: u64) {
        if self.intents.is_empty() {
            return;
        }
        let reference = self.book.mid();
        let mut intents = std::mem::take(&mut self.intents);
        for intent in intents.drain(..) {
            match intent {
                Intent::New { side, price, qty, tif } => {
                    if self.risk.check_new(side, price, qty, &self.oms, reference, self.now).is_err() {
                        continue;
                    }
                    let cl_id = self.oms.next_cl_id();
                    let ts = self.clock.now();
                    let msg =
                        OrderMsg { ts, instrument: self.inst.id, cmd: OrderCmd::New { cl_id, side, price, qty, tif } };
                    self.oms.on_new_sent(cl_id, side, price, qty, tif, ts);
                    self.venue.send(&msg);
                    self.latency(LatKind::TickToOrder, ts.saturating_sub(origin));
                    self.journal(Record::Order(msg));
                    self.counters.orders_sent += 1;
                }
                Intent::Cancel { cl_id } => {
                    // Cancels are never risk-blocked.
                    if self.oms.order(cl_id).is_some_and(|o| o.status != OrdStatus::PendingCancel) {
                        let msg = OrderMsg {
                            ts: self.clock.now(),
                            instrument: self.inst.id,
                            cmd: OrderCmd::Cancel { cl_id },
                        };
                        self.oms.on_cancel_sent(cl_id);
                        self.venue.send(&msg);
                        self.journal(Record::Order(msg));
                        self.counters.cancels_sent += 1;
                    }
                }
            }
        }
        self.intents = intents;
    }

    fn cancel_all(&mut self) {
        let ids: Vec<ClOrdId> =
            self.oms.orders().filter(|o| o.status != OrdStatus::PendingCancel).map(|o| o.cl_id).collect();
        self.intents.extend(ids.into_iter().map(|cl_id| Intent::Cancel { cl_id }));
        let now = self.clock.now();
        self.process_intents(now);
    }

    /// Low-frequency duties: kill switch, loss limit, stats snapshot.
    pub fn housekeeping(&mut self) {
        let now = self.clock.now();
        if self.risk.killed() {
            if !self.kill_handled {
                self.cancel_all();
                self.kill_handled = true;
            }
        } else {
            self.kill_handled = false;
        }
        if let Some(mid) = self.book.mid() {
            self.risk.check_pnl(self.oms.pnl(mid));
        }
        if self.out.stats.is_some() && now.saturating_sub(self.last_snapshot) >= SNAPSHOT_EVERY_NS {
            self.last_snapshot = now;
            let s = self.snapshot(now);
            if let Some(tx) = self.out.stats.as_mut() {
                let _ = tx.push(s);
            }
        }
    }

    pub fn snapshot(&self, now: u64) -> EngineStats {
        let mut s = self.counters;
        s.ts_ns = now;
        s.fills = self.oms.fills;
        s.maker_fills = self.oms.maker_fills;
        s.risk_rejects = self.risk.rejects;
        s.open_orders = self.oms.open_orders() as u64;
        s.position_lots = self.oms.position;
        s.volume_lots = self.oms.volume_lots;
        s.fees = self.oms.fees;
        s.killed = self.risk.killed();
        if let Some(mid) = self.book.mid() {
            s.pnl = self.oms.pnl(mid);
        }
        s.best_bid = self.book.best_bid().map_or(0, |l| l.price.0);
        s.best_ask = self.book.best_ask().map_or(0, |l| l.price.0);
        let (qb, qa) = self.strategy.quotes(&self.oms);
        s.our_bid = qb.map_or(0, |p| p.0);
        s.our_ask = qa.map_or(0, |p| p.0);
        let mut bids = [(0, 0); BOOK_LEVELS];
        let mut asks = [(0, 0); BOOK_LEVELS];
        self.book.top_n(Side::Buy, &mut bids);
        self.book.top_n(Side::Sell, &mut asks);
        s.bids = bids;
        s.asks = asks;
        s.open_buy_qty = self.oms.open_buy_qty;
        s.open_sell_qty = self.oms.open_sell_qty;
        s.rejects_by_reason = self.risk.rejects_by_reason;
        s.strat = self.strategy.diagnostics();
        for i in 0..TRADE_ROWS.min(self.trades_head) {
            s.trades[i] = self.trades[(self.trades_head - 1 - i) % TRADE_ROWS];
        }
        for i in 0..FILL_ROWS.min(self.fills_head) {
            s.fills_recent[i] = self.fills[(self.fills_head - 1 - i) % FILL_ROWS];
        }
        let mut orders: Vec<_> = self.oms.orders().collect();
        orders.sort_by_key(|o| std::cmp::Reverse(o.cl_id));
        for (row, o) in s.orders.iter_mut().zip(orders.into_iter().take(ORDER_ROWS)) {
            *row = OrderRow {
                cl_id: o.cl_id,
                side: o.side as u8,
                price: o.price.0,
                qty: o.qty.0,
                leaves: o.leaves.0,
                status: match o.status {
                    OrdStatus::PendingNew => 0,
                    OrdStatus::Live => 1,
                    OrdStatus::PendingCancel => 2,
                },
                age_ns: now.saturating_sub(o.sent_ts),
            };
        }
        s
    }

    /// Final state: cancels resting orders, emits a last snapshot.
    pub fn shutdown(&mut self) -> EngineStats {
        self.cancel_all();
        let now = self.clock.now();
        let s = self.snapshot(now);
        if let Some(tx) = self.out.stats.as_mut() {
            let _ = tx.push(s);
        }
        s
    }
}

/// Engine thread body: poll the market-data ring and the venue until `stop`.
pub fn run_loop<S: Strategy, V: Venue>(
    mut engine: Engine<S, V>,
    mut md_rx: Consumer<MdMsg>,
    stop: Arc<AtomicBool>,
) -> EngineStats {
    cpu::configure_current_thread(cpu::ThreadRole::LatencyCritical, engine.opts.core);
    let wait = engine.opts.wait;
    let mut idle = 0u32;
    let mut iter: u64 = 0;
    while !stop.load(Ordering::Relaxed) {
        let mut worked = false;
        for _ in 0..256 {
            match md_rx.pop() {
                Some(md) => {
                    engine.on_md(&md);
                    worked = true;
                }
                None => break,
            }
        }
        let now = engine.clock.now();
        engine.drain_venue(now);
        iter = iter.wrapping_add(1);
        if iter & 1023 == 0 {
            engine.housekeeping();
        }
        if worked {
            idle = 0;
        } else {
            wait.idle(&mut idle);
        }
    }
    engine.shutdown()
}
