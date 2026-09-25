//! Order-entry side of the engine. Every venue sits behind the one `Venue`
//! trait, so a strategy runs unchanged against the simulator, paper fills or a
//! real order gateway.

use tickrail_book::{L2Book, Level};
use tickrail_core::ring::{Consumer, Producer};
use tickrail_core::*;

pub trait Venue {
    fn send(&mut self, msg: &OrderMsg);
    /// Sees every market-data message after the book applied it.
    fn on_market(&mut self, _md: &MdMsg, _book: &L2Book) {}
    fn poll(&mut self, out: &mut Vec<ExecMsg>);
}

/// Order entry into the simulated exchange thread over SPSC rings.
pub struct SimGateway {
    pub tx: Producer<OrderMsg>,
    pub rx: Consumer<ExecMsg>,
}

impl Venue for SimGateway {
    #[inline]
    fn send(&mut self, msg: &OrderMsg) {
        self.tx.push_spin(*msg);
    }

    #[inline]
    fn poll(&mut self, out: &mut Vec<ExecMsg>) {
        while let Some(e) = self.rx.pop() {
            out.push(e);
        }
    }
}

#[derive(Copy, Clone, Debug)]
struct PaperOrder {
    cl_id: ClOrdId,
    side: Side,
    price: Price,
    leaves: i64,
}

/// In-process fill simulator for live market data and replays.
///
/// Conservative model: a resting order fills only when the market trades
/// *through* its price or the opposite touch crosses it (queue position at an
/// equal price is unknown, so we assume we are last). Aggressive orders fill
/// against the displayed top level only.
pub struct PaperVenue {
    instrument: InstrumentId,
    orders: Vec<PaperOrder>,
    pending: Vec<ExecMsg>,
    best_bid: Option<Level>,
    best_ask: Option<Level>,
    now: u64,
}

impl PaperVenue {
    pub fn new(instrument: InstrumentId) -> Self {
        PaperVenue {
            instrument,
            orders: Vec::with_capacity(64),
            pending: Vec::with_capacity(64),
            best_bid: None,
            best_ask: None,
            now: 0,
        }
    }

    fn report(&mut self, cl_id: ClOrdId, kind: ExecKind) {
        self.pending.push(ExecMsg { ts: self.now, instrument: self.instrument, cl_id, kind });
    }

    fn fill_resting(&mut self, i: usize, qty: i64) {
        let o = &mut self.orders[i];
        o.leaves -= qty;
        let (cl_id, price, leaves) = (o.cl_id, o.price, o.leaves);
        self.report(cl_id, ExecKind::Fill { price, qty: Qty(qty), leaves: Qty(leaves), liquidity: Liquidity::Maker });
    }
}

impl Venue for PaperVenue {
    fn send(&mut self, msg: &OrderMsg) {
        match msg.cmd {
            OrderCmd::New { cl_id, side, price, qty, tif } => {
                let opp = match side {
                    Side::Buy => self.best_ask,
                    Side::Sell => self.best_bid,
                };
                let crosses = opp.is_some_and(|l| match side {
                    Side::Buy => l.price <= price,
                    Side::Sell => l.price >= price,
                });
                if crosses && tif == TimeInForce::PostOnly {
                    self.report(cl_id, ExecKind::Rejected { reason: RejectReason::WouldCross });
                    return;
                }
                self.report(cl_id, ExecKind::Ack);
                let mut leaves = qty.0;
                if crosses {
                    let l = opp.expect("crosses implies level");
                    let take = leaves.min(l.qty.0);
                    leaves -= take;
                    self.report(
                        cl_id,
                        ExecKind::Fill {
                            price: l.price,
                            qty: Qty(take),
                            leaves: Qty(leaves),
                            liquidity: Liquidity::Taker,
                        },
                    );
                }
                if leaves > 0 {
                    if tif == TimeInForce::Ioc {
                        self.report(cl_id, ExecKind::Canceled);
                    } else {
                        self.orders.push(PaperOrder { cl_id, side, price, leaves });
                    }
                }
            }
            OrderCmd::Cancel { cl_id } => match self.orders.iter().position(|o| o.cl_id == cl_id) {
                Some(i) => {
                    self.orders.swap_remove(i);
                    self.report(cl_id, ExecKind::Canceled);
                }
                None => self.report(cl_id, ExecKind::Rejected { reason: RejectReason::UnknownOrder }),
            },
        }
    }

    fn on_market(&mut self, md: &MdMsg, book: &L2Book) {
        self.now = md.recv_ts;
        self.best_bid = book.best_bid();
        self.best_ask = book.best_ask();
        match md.kind {
            MdKind::Trade { aggressor, price, qty } => {
                let mut remaining = qty.0;
                let mut i = 0;
                while i < self.orders.len() && remaining > 0 {
                    let o = self.orders[i];
                    let traded_through = o.side == aggressor.opposite()
                        && match o.side {
                            Side::Buy => price < o.price,
                            Side::Sell => price > o.price,
                        };
                    if traded_through {
                        let q = o.leaves.min(remaining);
                        remaining -= q;
                        self.fill_resting(i, q);
                    }
                    i += 1;
                }
            }
            _ if md.is_last_in_batch() => {
                for i in 0..self.orders.len() {
                    let o = self.orders[i];
                    let crossed = match o.side {
                        Side::Buy => self.best_ask.is_some_and(|a| a.price <= o.price),
                        Side::Sell => self.best_bid.is_some_and(|b| b.price >= o.price),
                    };
                    if crossed && o.leaves > 0 {
                        self.fill_resting(i, o.leaves);
                    }
                }
            }
            _ => {}
        }
        self.orders.retain(|o| o.leaves > 0);
    }

    fn poll(&mut self, out: &mut Vec<ExecMsg>) {
        out.append(&mut self.pending);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(kind: MdKind) -> MdMsg {
        MdMsg { seq: 1, exch_ts: 0, recv_ts: 0, instrument: 1, flags: md_flags::LAST_IN_BATCH, kind }
    }

    #[test]
    fn paper_fills_only_on_trade_through() {
        let mut book = L2Book::default();
        book.set_level(Side::Buy, Price(100), Qty(5));
        book.set_level(Side::Sell, Price(102), Qty(5));
        let mut v = PaperVenue::new(1);
        v.on_market(&md(MdKind::Level { side: Side::Buy, price: Price(100), qty: Qty(5) }), &book);
        let new = |cl_id, side, px, tif| OrderMsg {
            ts: 0,
            instrument: 1,
            cmd: OrderCmd::New { cl_id, side, price: Price(px), qty: Qty(3), tif },
        };
        v.send(&new(1, Side::Buy, 101, TimeInForce::PostOnly));
        v.send(&new(2, Side::Buy, 102, TimeInForce::PostOnly)); // would cross → reject
        let mut out = Vec::new();
        v.poll(&mut out);
        assert_eq!(out[0].kind, ExecKind::Ack);
        assert!(matches!(out[1].kind, ExecKind::Rejected { reason: RejectReason::WouldCross }));
        out.clear();

        // Trade at our price: unknown queue position → no fill.
        v.on_market(&md(MdKind::Trade { aggressor: Side::Sell, price: Price(101), qty: Qty(10) }), &book);
        v.poll(&mut out);
        assert!(out.is_empty());
        // Trade through our price → filled.
        v.on_market(&md(MdKind::Trade { aggressor: Side::Sell, price: Price(100), qty: Qty(2) }), &book);
        v.poll(&mut out);
        assert!(matches!(out[0].kind, ExecKind::Fill { qty: Qty(2), leaves: Qty(1), liquidity: Liquidity::Maker, .. }));
    }
}
