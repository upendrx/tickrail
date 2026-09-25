//! Static dispatch over the built-in strategies. An enum instead of
//! `Box<dyn Strategy>` lets the compiler inline strategy code into the engine loop.

use crate::{Ctx, ImbalanceTaker, MarketMaker, Strategy};
use tickrail_core::*;
use tickrail_oms::{FillInfo, Oms};

pub enum AnyStrategy {
    MarketMaker(MarketMaker),
    Imbalance(ImbalanceTaker),
    /// Anything else (user plugins, language bindings), at the cost of a virtual call.
    Dyn(Box<dyn Strategy>),
}

macro_rules! dispatch {
    ($self:ident, $s:ident => $e:expr) => {
        match $self {
            AnyStrategy::MarketMaker($s) => $e,
            AnyStrategy::Imbalance($s) => $e,
            AnyStrategy::Dyn($s) => $e,
        }
    };
}

impl Strategy for AnyStrategy {
    fn name(&self) -> &'static str {
        dispatch!(self, s => s.name())
    }
    #[inline]
    fn on_book(&mut self, ctx: &mut Ctx) {
        dispatch!(self, s => s.on_book(ctx))
    }
    #[inline]
    fn on_trade(&mut self, ctx: &mut Ctx, price: Price, qty: Qty, aggressor: Side) {
        dispatch!(self, s => s.on_trade(ctx, price, qty, aggressor))
    }
    #[inline]
    fn on_fill(&mut self, ctx: &mut Ctx, fill: &FillInfo) {
        dispatch!(self, s => s.on_fill(ctx, fill))
    }
    fn diagnostic_names(&self) -> &'static [&'static str] {
        dispatch!(self, s => s.diagnostic_names())
    }
    fn diagnostics(&self) -> [f64; 8] {
        dispatch!(self, s => s.diagnostics())
    }
    fn quotes(&self, oms: &Oms) -> (Option<Price>, Option<Price>) {
        dispatch!(self, s => s.quotes(oms))
    }
}
