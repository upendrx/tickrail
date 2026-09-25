//! Strategies are plain structs called synchronously on the engine thread.
//!
//! A strategy sees a read-only view of the book and the OMS and returns
//! *intents* (new order, cancel). The engine owns order ids, risk checks and the
//! venue. Use `ctx.now` (event time) for anything time-based, never the wall
//! clock, otherwise a replay won't reproduce the live session.

mod any;
mod imbalance;
mod market_maker;
pub mod registry;

pub use any::AnyStrategy;
pub use imbalance::{ImbalanceParams, ImbalanceTaker};
pub use market_maker::{MarketMaker, MarketMakerParams};

use tickrail_book::L2Book;
use tickrail_core::*;
use tickrail_oms::{FillInfo, Oms, OrdStatus};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    New { side: Side, price: Price, qty: Qty, tif: TimeInForce },
    Cancel { cl_id: ClOrdId },
}

pub struct Ctx<'a> {
    /// Event time, ns.
    pub now: u64,
    pub book: &'a L2Book,
    pub oms: &'a Oms,
    pub intents: &'a mut Vec<Intent>,
}

pub trait Strategy: Send {
    fn name(&self) -> &'static str;

    /// Called once per consistent book state (end of a venue batch).
    fn on_book(&mut self, ctx: &mut Ctx);

    fn on_trade(&mut self, _ctx: &mut Ctx, _price: Price, _qty: Qty, _aggressor: Side) {}

    fn on_fill(&mut self, _ctx: &mut Ctx, _fill: &FillInfo) {}

    /// Names of the internal values exposed by `diagnostics` (max 8).
    fn diagnostic_names(&self) -> &'static [&'static str] {
        &[]
    }

    /// The strategy's current internal state, for display only.
    fn diagnostics(&self) -> [f64; 8] {
        [0.0; 8]
    }

    /// Our current (non-cancelling) quote prices, for display.
    fn quotes(&self, oms: &Oms) -> (Option<Price>, Option<Price>) {
        let best = |side: Side| {
            oms.orders().filter(|o| o.side == side && o.status != OrdStatus::PendingCancel).map(|o| o.price).reduce(
                |a, b| match side {
                    Side::Buy => a.max(b),
                    Side::Sell => a.min(b),
                },
            )
        };
        (best(Side::Buy), best(Side::Sell))
    }
}

impl Strategy for Box<dyn Strategy> {
    fn name(&self) -> &'static str {
        (**self).name()
    }
    fn on_book(&mut self, ctx: &mut Ctx) {
        (**self).on_book(ctx)
    }
    fn on_trade(&mut self, ctx: &mut Ctx, price: Price, qty: Qty, aggressor: Side) {
        (**self).on_trade(ctx, price, qty, aggressor)
    }
    fn on_fill(&mut self, ctx: &mut Ctx, fill: &FillInfo) {
        (**self).on_fill(ctx, fill)
    }
    fn diagnostic_names(&self) -> &'static [&'static str] {
        (**self).diagnostic_names()
    }
    fn diagnostics(&self) -> [f64; 8] {
        (**self).diagnostics()
    }
}
