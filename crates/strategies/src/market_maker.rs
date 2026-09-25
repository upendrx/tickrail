//! Inventory-aware market maker in the spirit of Avellaneda–Stoikov (2008):
//!
//! * fair value  = microprice (size-weighted mid);
//! * reservation = fair − inventory_ratio × skew  (lean away from inventory);
//! * half spread = base + k × σ (widen when the market moves fast);
//! * post-only quotes, re-quoted only when they drift ≥ `requote_ticks`
//!   (fewer cancels = keep queue priority and stay inside rate limits).

use crate::{Ctx, Intent, Strategy};
use tickrail_core::*;
use tickrail_oms::OrdStatus;

#[derive(Clone, Debug)]
pub struct MarketMakerParams {
    pub quote_qty: Qty,
    pub max_inventory: i64,
    pub base_half_spread_ticks: f64,
    pub vol_mult: f64,
    /// Reservation-price shift (ticks) when inventory is at its limit.
    pub skew_ticks_at_max: f64,
    pub requote_ticks: i64,
    /// Minimum event-time gap between new quotes on one side. Self-throttling
    /// keeps us well inside the risk rate limit instead of bouncing off it.
    pub min_requote_ns: u64,
}

pub struct MarketMaker {
    p: MarketMakerParams,
    last_mid: Option<f64>,
    var: f64,
    last_new: [u64; 2],
    diag: [f64; 8],
}

impl MarketMaker {
    pub fn new(p: MarketMakerParams) -> Self {
        MarketMaker { p, last_mid: None, var: 1.0, last_new: [0; 2], diag: [0.0; 8] }
    }

    fn manage_side(&mut self, ctx: &mut Ctx, side: Side, desired: Option<i64>) {
        let throttled = ctx.now.saturating_sub(self.last_new[side as usize]) < self.p.min_requote_ns;
        // While throttled, tolerate a somewhat stale quote rather than going dark.
        let tolerance = if throttled { self.p.requote_ticks * 4 } else { self.p.requote_ticks };
        let mut kept = false;
        let mut cancelling = 0;
        for o in ctx.oms.orders().filter(|o| o.side == side) {
            if o.status == OrdStatus::PendingCancel {
                cancelling += 1;
                continue;
            }
            match desired {
                Some(px) if !kept && (o.price.0 - px).abs() < tolerance => kept = true,
                _ => ctx.intents.push(Intent::Cancel { cl_id: o.cl_id }),
            }
        }
        if let Some(px) = desired
            && !kept
            && !throttled
            && cancelling < 2
        {
            ctx.intents.push(Intent::New { side, price: Price(px), qty: self.p.quote_qty, tif: TimeInForce::PostOnly });
            self.last_new[side as usize] = ctx.now;
        }
    }
}

impl Strategy for MarketMaker {
    fn name(&self) -> &'static str {
        "market_maker"
    }

    fn diagnostic_names(&self) -> &'static [&'static str] {
        &[
            "mid",
            "microprice",
            "reservation",
            "half_spread",
            "volatility",
            "inventory_ratio",
            "target_bid",
            "target_ask",
        ]
    }

    fn diagnostics(&self) -> [f64; 8] {
        self.diag
    }

    fn on_book(&mut self, ctx: &mut Ctx) {
        let (Some(bb), Some(ba), Some(micro), Some(mid)) =
            (ctx.book.best_bid(), ctx.book.best_ask(), ctx.book.microprice(), ctx.book.mid())
        else {
            return;
        };
        if let Some(last) = self.last_mid {
            let d = mid - last;
            self.var = 0.995 * self.var + 0.005 * d * d;
        }
        self.last_mid = Some(mid);

        let p = &self.p;
        let pos = ctx.oms.position;
        let inv_ratio = (pos as f64 / p.max_inventory as f64).clamp(-1.0, 1.0);
        let reservation = micro - inv_ratio * p.skew_ticks_at_max;
        let half = p.base_half_spread_ticks + p.vol_mult * self.var.sqrt();

        // Never cross: post-only would be rejected anyway, this saves the round trip.
        let bid = ((reservation - half).floor() as i64).min(ba.price.0 - 1);
        let ask = ((reservation + half).ceil() as i64).max(bb.price.0 + 1);
        let want_bid = pos + p.quote_qty.0 <= p.max_inventory;
        let want_ask = pos - p.quote_qty.0 >= -p.max_inventory;
        self.diag = [
            mid,
            micro,
            reservation,
            half,
            self.var.sqrt(),
            inv_ratio,
            if want_bid { bid as f64 } else { 0.0 },
            if want_ask { ask as f64 } else { 0.0 },
        ];

        self.manage_side(ctx, Side::Buy, want_bid.then_some(bid));
        self.manage_side(ctx, Side::Sell, want_ask.then_some(ask));
    }
}
