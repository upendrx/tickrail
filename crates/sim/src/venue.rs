//! Simulated venue thread: matching engine + background "market" + wire latency.
//!
//! Background flow is a zero-intelligence model around a latent fair value doing
//! a random walk: passive limit orders near the fair value, random cancels and
//! occasional aggressive orders. Because background limit orders are placed around
//! the *latent* value, a stale quote from our strategy gets picked off. The sim
//! has real adverse selection, which is what makes market-making hard.

use crate::matching::{MatchEvent, MatchingEngine, NewOrder};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tickrail_core::ring::{Consumer, Producer};
use tickrail_core::rng::Rng;
use tickrail_core::*;

/// Owner id of the strategy under test inside the matching engine.
pub const OUR_OWNER: u32 = 1;
const BG_OWNER: u32 = 2;
const BG_ID_BASE: u64 = 1 << 48;

#[derive(Clone, Debug)]
pub struct VenueConfig {
    pub instrument: InstrumentId,
    pub seed: u64,
    /// Background actions per second.
    pub bg_rate: f64,
    /// One-way order-entry latency (our order → matching engine), ns.
    pub order_latency_ns: u64,
    pub start_price: Price,
    /// Latent fair-value volatility, ticks per sqrt(second).
    pub vol_ticks_per_sqrt_s: f64,
    /// Background order size range in lots (1..=max).
    pub bg_max_qty: u64,
    pub max_bg_orders: usize,
    pub wait: WaitStrategy,
    /// Core to pin the venue thread to (Linux only).
    pub core: Option<usize>,
}

impl Default for VenueConfig {
    fn default() -> Self {
        VenueConfig {
            instrument: 1,
            seed: 42,
            bg_rate: 20_000.0,
            order_latency_ns: 20_000,
            start_price: Price(6_000_000),
            vol_ticks_per_sqrt_s: 150.0,
            bg_max_qty: 20,
            max_bg_orders: 3_000,
            wait: WaitStrategy::Backoff,
            core: None,
        }
    }
}

pub struct VenueSim {
    cfg: VenueConfig,
    clock: Clock,
    engine: MatchingEngine,
    rng: Rng,
    fair: f64,
    sigma_per_action: f64,
    next_bg_id: u64,
    bg_orders: Vec<u64>,
    actions: u64,
    seq: u64,
    events: Vec<MatchEvent>,
    inflight: VecDeque<(u64, OrderMsg)>,
    md_tx: Producer<MdMsg>,
    exec_tx: Producer<ExecMsg>,
    order_rx: Consumer<OrderMsg>,
    pub md_dropped: u64,
}

impl VenueSim {
    pub fn new(
        cfg: VenueConfig,
        clock: Clock,
        md_tx: Producer<MdMsg>,
        exec_tx: Producer<ExecMsg>,
        order_rx: Consumer<OrderMsg>,
    ) -> Self {
        let sigma_per_action = cfg.vol_ticks_per_sqrt_s / cfg.bg_rate.sqrt();
        VenueSim {
            rng: Rng::new(cfg.seed),
            fair: cfg.start_price.0 as f64,
            sigma_per_action,
            cfg,
            clock,
            engine: MatchingEngine::new(),
            next_bg_id: BG_ID_BASE,
            bg_orders: Vec::with_capacity(8192),
            actions: 0,
            seq: 0,
            events: Vec::with_capacity(256),
            inflight: VecDeque::with_capacity(1024),
            md_tx,
            exec_tx,
            order_rx,
            md_dropped: 0,
        }
    }

    /// Venue thread main loop. Busy-spins until `stop`.
    pub fn run(mut self, stop: Arc<AtomicBool>) {
        cpu::configure_current_thread(cpu::ThreadRole::LatencyCritical, self.cfg.core);
        let wait = self.cfg.wait;
        let mut idle = 0u32;
        // Build an initial book with passive-only flow.
        for _ in 0..600 {
            self.bg_passive();
            self.publish();
        }
        let mut next_bg = self.clock.now() as f64;
        let mean_gap_ns = 1e9 / self.cfg.bg_rate;
        while !stop.load(Ordering::Relaxed) {
            let now = self.clock.now();
            let mut worked = false;

            while let Some(msg) = self.order_rx.pop() {
                self.inflight.push_back((msg.ts + self.cfg.order_latency_ns, msg));
            }
            while let Some(&(due, msg)) = self.inflight.front() {
                if due > now {
                    break;
                }
                self.inflight.pop_front();
                self.handle_our_order(msg);
                self.publish();
                worked = true;
            }
            // Catch up background flow to the current time (Poisson arrivals).
            let mut burst = 0;
            while next_bg <= now as f64 && burst < 64 {
                self.bg_action();
                self.publish();
                next_bg += self.rng.exp(1.0) * mean_gap_ns;
                burst += 1;
                worked = true;
            }
            if worked {
                idle = 0;
            } else if self.inflight.is_empty() {
                wait.idle(&mut idle);
            } else {
                // Orders are in flight on the simulated wire: stay responsive.
                std::hint::spin_loop();
            }
        }
    }

    fn handle_our_order(&mut self, msg: OrderMsg) {
        match msg.cmd {
            OrderCmd::New { cl_id, side, price, qty, tif } => {
                self.engine.submit(NewOrder { owner: OUR_OWNER, id: cl_id, side, price, qty, tif }, &mut self.events)
            }
            OrderCmd::Cancel { cl_id } => self.engine.cancel(OUR_OWNER, cl_id, &mut self.events),
        }
    }

    fn bg_action(&mut self) {
        self.actions += 1;
        self.fair += self.rng.normal() * self.sigma_per_action;
        if self.actions.is_multiple_of(20_000) {
            let engine = &self.engine;
            self.bg_orders.retain(|id| engine.contains(*id));
        }
        let u = self.rng.f64();
        if u < 0.52 && self.bg_orders.len() < self.cfg.max_bg_orders {
            self.bg_passive();
        } else if u < 0.92 {
            if !self.bg_orders.is_empty() {
                let i = self.rng.below(self.bg_orders.len() as u64) as usize;
                let id = self.bg_orders.swap_remove(i);
                self.engine.cancel(BG_OWNER, id, &mut self.events);
                // Unknown (already filled) orders produce a reject we simply ignore.
            }
        } else {
            self.bg_aggressive();
        }
    }

    /// Limit order a geometric distance behind the latent fair value.
    /// If fair value has moved through the book this order crosses → trades.
    fn bg_passive(&mut self) {
        let side = if self.rng.coin() { Side::Buy } else { Side::Sell };
        let dist = self.rng.geometric(0.25) as i64;
        let price = match side {
            Side::Buy => self.fair.floor() as i64 - dist,
            Side::Sell => self.fair.ceil() as i64 + dist,
        };
        let id = self.next_id();
        let qty = 1 + self.rng.below(self.cfg.bg_max_qty) as i64;
        self.engine.submit(
            NewOrder { owner: BG_OWNER, id, side, price: Price(price), qty: Qty(qty), tif: TimeInForce::Gtc },
            &mut self.events,
        );
        self.bg_orders.push(id);
    }

    fn bg_aggressive(&mut self) {
        // Informed-ish: trade in the direction of fair value vs the touch.
        let mid = match (self.engine.best_bid(), self.engine.best_ask()) {
            (Some(b), Some(a)) => (b.0 + a.0) as f64 * 0.5,
            _ => self.fair,
        };
        let side = if self.rng.f64() < 0.5 + (self.fair - mid).clamp(-2.0, 2.0) * 0.2 { Side::Buy } else { Side::Sell };
        let limit = match side {
            Side::Buy => self.fair.ceil() as i64 + 3,
            Side::Sell => self.fair.floor() as i64 - 3,
        };
        let id = self.next_id();
        let qty = 1 + self.rng.below(self.cfg.bg_max_qty * 2) as i64;
        self.engine.submit(
            NewOrder { owner: BG_OWNER, id, side, price: Price(limit), qty: Qty(qty), tif: TimeInForce::Ioc },
            &mut self.events,
        );
    }

    fn next_id(&mut self) -> u64 {
        self.next_bg_id += 1;
        self.next_bg_id
    }

    /// Converts matching events into public market data and our private execs.
    fn publish(&mut self) {
        if self.events.is_empty() {
            return;
        }
        let now = self.clock.now();
        let last_public =
            self.events.iter().rposition(|e| matches!(e, MatchEvent::Level { .. } | MatchEvent::Trade { .. }));
        for (i, ev) in self.events.drain(..).enumerate() {
            let md = match ev {
                MatchEvent::Level { side, price, qty } => Some(MdKind::Level { side, price, qty }),
                MatchEvent::Trade { price, qty, aggressor } => Some(MdKind::Trade { aggressor, price, qty }),
                _ => None,
            };
            if let Some(kind) = md {
                self.seq += 1;
                let flags = if Some(i) == last_public { md_flags::LAST_IN_BATCH } else { 0 };
                let msg =
                    MdMsg { seq: self.seq, exch_ts: now, recv_ts: now, instrument: self.cfg.instrument, flags, kind };
                // A real feed never waits for a slow consumer: drop and count (gap).
                if self.md_tx.push(msg).is_err() {
                    self.md_dropped += 1;
                }
                continue;
            }
            let (owner, id, kind) = match ev {
                MatchEvent::Accepted { owner, id } => (owner, id, ExecKind::Ack),
                MatchEvent::Rejected { owner, id, reason } => (owner, id, ExecKind::Rejected { reason }),
                MatchEvent::Fill { owner, id, price, qty, leaves, liquidity } => {
                    (owner, id, ExecKind::Fill { price, qty, leaves, liquidity })
                }
                MatchEvent::Canceled { owner, id } => (owner, id, ExecKind::Canceled),
                _ => unreachable!(),
            };
            if owner == OUR_OWNER {
                // Private execs must never be lost: apply back-pressure instead.
                self.exec_tx.push_spin(ExecMsg { ts: now, instrument: self.cfg.instrument, cl_id: id, kind });
            }
        }
    }
}
