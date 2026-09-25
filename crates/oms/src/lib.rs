//! Order management: the engine's single source of truth about its own orders,
//! position and PnL. Updated synchronously on the engine thread, so no locks.

use std::collections::HashMap;
use tickrail_core::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OrdStatus {
    /// Sent, not yet acknowledged.
    PendingNew,
    Live,
    PendingCancel,
}

#[derive(Copy, Clone, Debug)]
pub struct Order {
    pub cl_id: ClOrdId,
    pub side: Side,
    pub price: Price,
    pub qty: Qty,
    pub leaves: Qty,
    pub tif: TimeInForce,
    pub status: OrdStatus,
    /// Venue has acknowledged the order (distinguishes new-rejects from cancel-rejects).
    pub acked: bool,
    pub sent_ts: u64,
}

#[derive(Copy, Clone, Debug)]
pub struct FillInfo {
    pub cl_id: ClOrdId,
    pub side: Side,
    pub price: Price,
    pub qty: Qty,
    pub liquidity: Liquidity,
}

#[derive(Clone, Debug)]
pub struct FeeSchedule {
    /// Negative = rebate.
    pub maker_bps: f64,
    pub taker_bps: f64,
}

pub struct Oms {
    orders: HashMap<ClOrdId, Order>,
    next_cl_id: ClOrdId,
    /// Signed position in lots.
    pub position: i64,
    /// Cash in (tick × lot) units: selling adds, buying subtracts.
    pub cash: i64,
    /// Fees paid in quote currency.
    pub fees: f64,
    pub open_buy_qty: i64,
    pub open_sell_qty: i64,
    pub fills: u64,
    pub maker_fills: u64,
    pub volume_lots: i64,
    unit: f64,
    fee_sched: FeeSchedule,
}

impl Oms {
    pub fn new(instrument: &Instrument, fee_sched: FeeSchedule) -> Self {
        Oms {
            orders: HashMap::with_capacity(1024),
            next_cl_id: 1,
            position: 0,
            cash: 0,
            fees: 0.0,
            open_buy_qty: 0,
            open_sell_qty: 0,
            fills: 0,
            maker_fills: 0,
            volume_lots: 0,
            unit: instrument.notional_unit(),
            fee_sched,
        }
    }

    pub fn next_cl_id(&mut self) -> ClOrdId {
        let id = self.next_cl_id;
        self.next_cl_id += 1;
        id
    }

    pub fn orders(&self) -> impl Iterator<Item = &Order> {
        self.orders.values()
    }

    pub fn order(&self, cl_id: ClOrdId) -> Option<&Order> {
        self.orders.get(&cl_id)
    }

    pub fn open_orders(&self) -> usize {
        self.orders.len()
    }

    fn open_qty_mut(&mut self, side: Side) -> &mut i64 {
        match side {
            Side::Buy => &mut self.open_buy_qty,
            Side::Sell => &mut self.open_sell_qty,
        }
    }

    pub fn on_new_sent(&mut self, cl_id: ClOrdId, side: Side, price: Price, qty: Qty, tif: TimeInForce, ts: u64) {
        self.orders.insert(
            cl_id,
            Order {
                cl_id,
                side,
                price,
                qty,
                leaves: qty,
                tif,
                status: OrdStatus::PendingNew,
                acked: false,
                sent_ts: ts,
            },
        );
        *self.open_qty_mut(side) += qty.0;
    }

    pub fn on_cancel_sent(&mut self, cl_id: ClOrdId) {
        if let Some(o) = self.orders.get_mut(&cl_id) {
            o.status = OrdStatus::PendingCancel;
        }
    }

    fn remove(&mut self, cl_id: ClOrdId) {
        if let Some(o) = self.orders.remove(&cl_id) {
            *self.open_qty_mut(o.side) -= o.leaves.0;
        }
    }

    /// Applies an execution report. Returns fill details when it was a fill.
    pub fn on_exec(&mut self, e: &ExecMsg) -> Option<FillInfo> {
        match e.kind {
            ExecKind::Ack => {
                if let Some(o) = self.orders.get_mut(&e.cl_id) {
                    o.acked = true;
                    if o.status == OrdStatus::PendingNew {
                        o.status = OrdStatus::Live;
                    }
                }
                None
            }
            ExecKind::Fill { price, qty, leaves, liquidity } => {
                let o = self.orders.get_mut(&e.cl_id)?;
                let side = o.side;
                o.leaves = leaves;
                let done = leaves.0 == 0;
                *self.open_qty_mut(side) -= qty.0;
                if done {
                    self.orders.remove(&e.cl_id);
                }
                self.position += side.sign() * qty.0;
                self.cash -= side.sign() * qty.0 * price.0;
                let notional = (qty.0 * price.0) as f64 * self.unit;
                let bps = match liquidity {
                    Liquidity::Maker => self.fee_sched.maker_bps,
                    Liquidity::Taker => self.fee_sched.taker_bps,
                };
                self.fees += notional * bps * 1e-4;
                self.fills += 1;
                if liquidity == Liquidity::Maker {
                    self.maker_fills += 1;
                }
                self.volume_lots += qty.0;
                Some(FillInfo { cl_id: e.cl_id, side, price, qty, liquidity })
            }
            ExecKind::Canceled => {
                self.remove(e.cl_id);
                None
            }
            ExecKind::Rejected { reason } => {
                match self.orders.get_mut(&e.cl_id) {
                    // Never acked → the new order itself was rejected (even if a
                    // cancel for it was already in flight).
                    Some(o) if !o.acked => self.remove(e.cl_id),
                    // Cancel rejected because the venue no longer has the order:
                    // venues report fills before the cancel-reject, so it is gone.
                    Some(_) if reason == RejectReason::UnknownOrder => self.remove(e.cl_id),
                    // Cancel rejected for another reason: order still working.
                    Some(o) => o.status = OrdStatus::Live,
                    None => {}
                }
                None
            }
        }
    }

    /// Mark-to-market PnL in quote currency, net of fees.
    pub fn pnl(&self, mark_ticks: f64) -> f64 {
        (self.cash as f64 + self.position as f64 * mark_ticks) * self.unit - self.fees
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst() -> Instrument {
        Instrument { id: 1, symbol: "T".into(), price_decimals: 2, qty_decimals: 3 }
    }

    fn exec(cl_id: u64, kind: ExecKind) -> ExecMsg {
        ExecMsg { ts: 0, instrument: 1, cl_id, kind }
    }

    #[test]
    fn round_trip_pnl() {
        let mut oms = Oms::new(&inst(), FeeSchedule { maker_bps: 0.0, taker_bps: 0.0 });
        let b = oms.next_cl_id();
        oms.on_new_sent(b, Side::Buy, Price(10_000), Qty(1_000), TimeInForce::Gtc, 0);
        assert_eq!(oms.open_buy_qty, 1_000);
        oms.on_exec(&exec(b, ExecKind::Ack));
        oms.on_exec(&exec(
            b,
            ExecKind::Fill { price: Price(10_000), qty: Qty(400), leaves: Qty(600), liquidity: Liquidity::Maker },
        ));
        assert_eq!(oms.position, 400);
        assert_eq!(oms.open_buy_qty, 600);
        oms.on_exec(&exec(b, ExecKind::Canceled));
        assert_eq!(oms.open_buy_qty, 0);
        assert_eq!(oms.open_orders(), 0);

        let s = oms.next_cl_id();
        oms.on_new_sent(s, Side::Sell, Price(10_050), Qty(400), TimeInForce::Gtc, 0);
        oms.on_exec(&exec(
            s,
            ExecKind::Fill { price: Price(10_050), qty: Qty(400), leaves: Qty(0), liquidity: Liquidity::Maker },
        ));
        assert_eq!(oms.position, 0);
        // Bought 0.4 @ 100.00, sold 0.4 @ 100.50 → +0.20
        assert!((oms.pnl(10_000.0) - 0.20).abs() < 1e-9);
    }

    #[test]
    fn rejects_restore_state() {
        let mut oms = Oms::new(&inst(), FeeSchedule { maker_bps: 0.0, taker_bps: 2.0 });
        let id = oms.next_cl_id();
        oms.on_new_sent(id, Side::Sell, Price(1), Qty(5), TimeInForce::PostOnly, 0);
        oms.on_exec(&exec(id, ExecKind::Rejected { reason: RejectReason::WouldCross }));
        assert_eq!(oms.open_sell_qty, 0);
        assert!(oms.order(id).is_none());

        let id = oms.next_cl_id();
        oms.on_new_sent(id, Side::Buy, Price(1), Qty(5), TimeInForce::Gtc, 0);
        oms.on_exec(&exec(id, ExecKind::Ack));
        oms.on_cancel_sent(id);
        oms.on_exec(&exec(id, ExecKind::Rejected { reason: RejectReason::InvalidOrder }));
        assert_eq!(oms.order(id).unwrap().status, OrdStatus::Live);
        oms.on_cancel_sent(id);
        oms.on_exec(&exec(id, ExecKind::Rejected { reason: RejectReason::UnknownOrder }));
        assert!(oms.order(id).is_none(), "venue no longer has it");
    }

    #[test]
    fn new_reject_after_cancel_sent_leaves_no_ghost() {
        let mut oms = Oms::new(&inst(), FeeSchedule { maker_bps: 0.0, taker_bps: 0.0 });
        let id = oms.next_cl_id();
        oms.on_new_sent(id, Side::Buy, Price(10), Qty(5), TimeInForce::PostOnly, 0);
        oms.on_cancel_sent(id);
        oms.on_exec(&exec(id, ExecKind::Rejected { reason: RejectReason::WouldCross }));
        assert!(oms.order(id).is_none());
        assert_eq!(oms.open_buy_qty, 0);
    }
}
