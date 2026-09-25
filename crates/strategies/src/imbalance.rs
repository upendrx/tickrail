//! Liquidity-taking example: when the top of book is heavily one-sided, the next
//! price move is more likely in that direction, so cross the spread with an IOC.
//! Flattens inventory when the signal reverses.

use crate::{Ctx, Intent, Strategy};
use tickrail_core::*;

#[derive(Clone, Debug)]
pub struct ImbalanceParams {
    pub levels: usize,
    pub threshold: f64,
    pub qty: Qty,
    pub max_inventory: i64,
    pub cooldown_ns: u64,
}

pub struct ImbalanceTaker {
    p: ImbalanceParams,
    last_order_ts: u64,
    last_imbalance: f64,
}

impl ImbalanceTaker {
    pub fn new(p: ImbalanceParams) -> Self {
        ImbalanceTaker { p, last_order_ts: 0, last_imbalance: 0.0 }
    }
}

impl Strategy for ImbalanceTaker {
    fn name(&self) -> &'static str {
        "imbalance_taker"
    }

    fn diagnostic_names(&self) -> &'static [&'static str] {
        &["imbalance", "threshold", "cooldown_ms"]
    }

    fn diagnostics(&self) -> [f64; 8] {
        [self.last_imbalance, self.p.threshold, self.p.cooldown_ns as f64 / 1e6, 0.0, 0.0, 0.0, 0.0, 0.0]
    }

    fn on_book(&mut self, ctx: &mut Ctx) {
        self.last_imbalance = ctx.book.imbalance(self.p.levels);
        if ctx.now.saturating_sub(self.last_order_ts) < self.p.cooldown_ns || ctx.oms.open_orders() > 0 {
            return;
        }
        let (Some(bb), Some(ba)) = (ctx.book.best_bid(), ctx.book.best_ask()) else { return };
        let imb = ctx.book.imbalance(self.p.levels);
        let pos = ctx.oms.position;
        let order = if imb > self.p.threshold && pos + self.p.qty.0 <= self.p.max_inventory {
            Some((Side::Buy, ba.price, self.p.qty))
        } else if imb < -self.p.threshold && pos - self.p.qty.0 >= -self.p.max_inventory {
            Some((Side::Sell, bb.price, self.p.qty))
        } else if imb.abs() < 0.1 && pos != 0 {
            // Signal gone: flatten.
            let side = if pos > 0 { Side::Sell } else { Side::Buy };
            let px = if pos > 0 { bb.price } else { ba.price };
            Some((side, px, Qty(pos.abs().min(self.p.qty.0))))
        } else {
            None
        };
        if let Some((side, price, qty)) = order {
            ctx.intents.push(Intent::New { side, price, qty, tif: TimeInForce::Ioc });
            self.last_order_ts = ctx.now;
        }
    }
}
