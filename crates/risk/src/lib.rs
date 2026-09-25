//! Pre-trade risk, run inline on the engine thread before every order leaves.
//!
//! Follows the usual rules, which SEC 15c3-5 and MiFID II RTS 6 also require:
//! checks are synchronous and can't be bypassed, **cancels are never blocked**,
//! and a kill switch can flatten everything from outside the trading thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tickrail_core::*;
use tickrail_oms::Oms;

#[derive(Clone, Debug)]
pub struct RiskLimits {
    pub max_order_qty: Qty,
    /// Worst-case |position| if every open order on one side filled.
    pub max_position: i64,
    pub max_open_orders: usize,
    /// Max distance of an order price from the reference (mid), in ticks.
    pub price_band_ticks: i64,
    pub max_orders_per_sec: u32,
    /// Trip the kill switch when PnL (quote ccy) falls below `-max_loss`.
    pub max_loss: f64,
}

pub struct RiskEngine {
    pub limits: RiskLimits,
    kill: Arc<AtomicBool>,
    window_start: u64,
    window_count: u32,
    pub rejects: u64,
    /// Same order as the dashboard's reason list.
    pub rejects_by_reason: [u64; 8],
}

fn reason_index(r: RejectReason) -> usize {
    match r {
        RejectReason::RiskMaxQty => 0,
        RejectReason::RiskMaxPosition => 1,
        RejectReason::RiskPriceBand => 2,
        RejectReason::RiskRateLimit => 3,
        RejectReason::RiskMaxOpenOrders => 4,
        RejectReason::RiskKillSwitch => 5,
        RejectReason::RiskNoReference => 6,
        _ => 7,
    }
}

impl RiskEngine {
    pub fn new(limits: RiskLimits, kill: Arc<AtomicBool>) -> Self {
        RiskEngine { limits, kill, window_start: 0, window_count: 0, rejects: 0, rejects_by_reason: [0; 8] }
    }

    pub fn killed(&self) -> bool {
        self.kill.load(Ordering::Relaxed)
    }

    pub fn trip(&self) {
        self.kill.store(true, Ordering::Relaxed);
    }

    /// `reference`: current mid in ticks, if the book is valid.
    #[inline]
    pub fn check_new(
        &mut self,
        side: Side,
        price: Price,
        qty: Qty,
        oms: &Oms,
        reference: Option<f64>,
        now: u64,
    ) -> Result<(), RejectReason> {
        let r = self.check_inner(side, price, qty, oms, reference, now);
        if let Err(reason) = r {
            self.rejects += 1;
            self.rejects_by_reason[reason_index(reason)] += 1;
        }
        r
    }

    fn check_inner(
        &mut self,
        side: Side,
        price: Price,
        qty: Qty,
        oms: &Oms,
        reference: Option<f64>,
        now: u64,
    ) -> Result<(), RejectReason> {
        let l = &self.limits;
        if self.kill.load(Ordering::Relaxed) {
            return Err(RejectReason::RiskKillSwitch);
        }
        if qty.0 <= 0 || qty > l.max_order_qty {
            return Err(RejectReason::RiskMaxQty);
        }
        if oms.open_orders() >= l.max_open_orders {
            return Err(RejectReason::RiskMaxOpenOrders);
        }
        let worst = match side {
            Side::Buy => oms.position + oms.open_buy_qty + qty.0,
            Side::Sell => oms.position - oms.open_sell_qty - qty.0,
        };
        if worst.abs() > l.max_position {
            return Err(RejectReason::RiskMaxPosition);
        }
        let Some(mid) = reference else { return Err(RejectReason::RiskNoReference) };
        if (price.0 as f64 - mid).abs() > l.price_band_ticks as f64 {
            return Err(RejectReason::RiskPriceBand);
        }
        // Fixed 1-second window order-rate throttle (event-time based: replay-safe).
        if now.saturating_sub(self.window_start) >= 1_000_000_000 {
            self.window_start = now;
            self.window_count = 0;
        }
        if self.window_count >= l.max_orders_per_sec {
            return Err(RejectReason::RiskRateLimit);
        }
        self.window_count += 1;
        Ok(())
    }

    /// Post-trade check, called on every fill / periodically.
    pub fn check_pnl(&self, pnl: f64) {
        if pnl < -self.limits.max_loss {
            self.trip();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tickrail_oms::FeeSchedule;

    fn setup() -> (RiskEngine, Oms) {
        let inst = Instrument { id: 1, symbol: "T".into(), price_decimals: 2, qty_decimals: 3 };
        let limits = RiskLimits {
            max_order_qty: Qty(10),
            max_position: 15,
            max_open_orders: 4,
            price_band_ticks: 50,
            max_orders_per_sec: 3,
            max_loss: 100.0,
        };
        (
            RiskEngine::new(limits, Arc::new(AtomicBool::new(false))),
            Oms::new(&inst, FeeSchedule { maker_bps: 0.0, taker_bps: 0.0 }),
        )
    }

    #[test]
    fn enforces_limits() {
        let (mut r, mut oms) = setup();
        let mid = Some(1000.0);
        assert_eq!(r.check_new(Side::Buy, Price(1000), Qty(11), &oms, mid, 0), Err(RejectReason::RiskMaxQty));
        assert_eq!(r.check_new(Side::Buy, Price(1100), Qty(1), &oms, mid, 0), Err(RejectReason::RiskPriceBand));
        assert_eq!(r.check_new(Side::Buy, Price(1000), Qty(1), &oms, None, 0), Err(RejectReason::RiskNoReference));
        oms.on_new_sent(1, Side::Buy, Price(1000), Qty(10), TimeInForce::Gtc, 0);
        assert_eq!(r.check_new(Side::Buy, Price(1000), Qty(6), &oms, mid, 0), Err(RejectReason::RiskMaxPosition));
        assert_eq!(r.check_new(Side::Sell, Price(1000), Qty(6), &oms, mid, 0), Ok(()));
        assert_eq!(r.check_new(Side::Sell, Price(1000), Qty(1), &oms, mid, 10), Ok(()));
        assert_eq!(r.check_new(Side::Sell, Price(1000), Qty(1), &oms, mid, 20), Ok(()));
        assert_eq!(r.check_new(Side::Sell, Price(1000), Qty(1), &oms, mid, 30), Err(RejectReason::RiskRateLimit));
        assert_eq!(r.check_new(Side::Sell, Price(1000), Qty(1), &oms, mid, 1_000_000_001), Ok(()));
        r.check_pnl(-101.0);
        assert_eq!(
            r.check_new(Side::Sell, Price(1000), Qty(1), &oms, mid, 2_000_000_000),
            Err(RejectReason::RiskKillSwitch)
        );
    }
}
