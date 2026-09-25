//! Central limit order book with price-time (FIFO) priority.

use std::collections::{BTreeMap, HashMap, VecDeque};
use tickrail_core::{Liquidity, Price, Qty, RejectReason, Side, TimeInForce};

pub type OwnerId = u32;

#[derive(Copy, Clone, Debug)]
pub struct NewOrder {
    pub owner: OwnerId,
    pub id: u64,
    pub side: Side,
    pub price: Price,
    pub qty: Qty,
    pub tif: TimeInForce,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MatchEvent {
    Accepted {
        owner: OwnerId,
        id: u64,
    },
    Rejected {
        owner: OwnerId,
        id: u64,
        reason: RejectReason,
    },
    Fill {
        owner: OwnerId,
        id: u64,
        price: Price,
        qty: Qty,
        leaves: Qty,
        liquidity: Liquidity,
    },
    Canceled {
        owner: OwnerId,
        id: u64,
    },
    /// Public print.
    Trade {
        price: Price,
        qty: Qty,
        aggressor: Side,
    },
    /// Public: new aggregate quantity at a level (0 = level gone).
    Level {
        side: Side,
        price: Price,
        qty: Qty,
    },
}

#[derive(Debug)]
struct Resting {
    owner: OwnerId,
    side: Side,
    price: Price,
    leaves: i64,
}

#[derive(Debug, Default)]
struct PriceLevel {
    total: i64,
    queue: VecDeque<u64>,
}

#[derive(Default)]
pub struct MatchingEngine {
    bids: BTreeMap<i64, PriceLevel>,
    asks: BTreeMap<i64, PriceLevel>,
    orders: HashMap<u64, Resting>,
}

impl MatchingEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn best_bid(&self) -> Option<Price> {
        self.bids.last_key_value().map(|(p, _)| Price(*p))
    }

    pub fn best_ask(&self) -> Option<Price> {
        self.asks.first_key_value().map(|(p, _)| Price(*p))
    }

    pub fn contains(&self, id: u64) -> bool {
        self.orders.contains_key(&id)
    }

    pub fn resting_orders(&self) -> usize {
        self.orders.len()
    }

    fn crosses(side: Side, limit: Price, opposite_best: Option<Price>) -> bool {
        match (side, opposite_best) {
            (Side::Buy, Some(a)) => a <= limit,
            (Side::Sell, Some(b)) => b >= limit,
            (_, None) => false,
        }
    }

    pub fn submit(&mut self, o: NewOrder, out: &mut Vec<MatchEvent>) {
        if o.qty.0 <= 0 || self.orders.contains_key(&o.id) {
            out.push(MatchEvent::Rejected { owner: o.owner, id: o.id, reason: RejectReason::InvalidOrder });
            return;
        }
        let opposite_best = match o.side {
            Side::Buy => self.best_ask(),
            Side::Sell => self.best_bid(),
        };
        if o.tif == TimeInForce::PostOnly && Self::crosses(o.side, o.price, opposite_best) {
            out.push(MatchEvent::Rejected { owner: o.owner, id: o.id, reason: RejectReason::WouldCross });
            return;
        }
        out.push(MatchEvent::Accepted { owner: o.owner, id: o.id });

        let mut leaves = o.qty.0;
        let (book, orders) = match o.side {
            Side::Buy => (&mut self.asks, &mut self.orders),
            Side::Sell => (&mut self.bids, &mut self.orders),
        };
        while leaves > 0 {
            let best = match o.side {
                Side::Buy => book.first_key_value().map(|(p, _)| *p),
                Side::Sell => book.last_key_value().map(|(p, _)| *p),
            };
            let Some(px) = best else { break };
            if !Self::crosses(o.side, o.price, Some(Price(px))) {
                break;
            }
            let level = book.get_mut(&px).expect("level exists");
            while leaves > 0 {
                let Some(&maker_id) = level.queue.front() else { break };
                let maker = orders.get_mut(&maker_id).expect("queued order is resting");
                let q = leaves.min(maker.leaves);
                maker.leaves -= q;
                leaves -= q;
                level.total -= q;
                out.push(MatchEvent::Fill {
                    owner: maker.owner,
                    id: maker_id,
                    price: Price(px),
                    qty: Qty(q),
                    leaves: Qty(maker.leaves),
                    liquidity: Liquidity::Maker,
                });
                out.push(MatchEvent::Fill {
                    owner: o.owner,
                    id: o.id,
                    price: Price(px),
                    qty: Qty(q),
                    leaves: Qty(leaves),
                    liquidity: Liquidity::Taker,
                });
                out.push(MatchEvent::Trade { price: Price(px), qty: Qty(q), aggressor: o.side });
                if maker.leaves == 0 {
                    level.queue.pop_front();
                    orders.remove(&maker_id);
                }
            }
            out.push(MatchEvent::Level { side: o.side.opposite(), price: Price(px), qty: Qty(level.total) });
            if level.queue.is_empty() {
                book.remove(&px);
            }
        }

        if leaves > 0 {
            if o.tif == TimeInForce::Ioc {
                out.push(MatchEvent::Canceled { owner: o.owner, id: o.id });
            } else {
                let own = match o.side {
                    Side::Buy => &mut self.bids,
                    Side::Sell => &mut self.asks,
                };
                let level = own.entry(o.price.0).or_default();
                level.total += leaves;
                level.queue.push_back(o.id);
                let total = level.total;
                self.orders.insert(o.id, Resting { owner: o.owner, side: o.side, price: o.price, leaves });
                out.push(MatchEvent::Level { side: o.side, price: o.price, qty: Qty(total) });
            }
        }
    }

    pub fn cancel(&mut self, owner: OwnerId, id: u64, out: &mut Vec<MatchEvent>) {
        match self.orders.get(&id) {
            Some(r) if r.owner == owner => {}
            _ => {
                out.push(MatchEvent::Rejected { owner, id, reason: RejectReason::UnknownOrder });
                return;
            }
        }
        let r = self.orders.remove(&id).expect("checked above");
        let book = match r.side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        if let Some(level) = book.get_mut(&r.price.0) {
            if let Some(pos) = level.queue.iter().position(|&x| x == id) {
                level.queue.remove(pos);
            }
            level.total -= r.leaves;
            let total = level.total;
            if level.queue.is_empty() {
                book.remove(&r.price.0);
            }
            out.push(MatchEvent::Level { side: r.side, price: r.price, qty: Qty(total.max(0)) });
        }
        out.push(MatchEvent::Canceled { owner, id });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(owner: u32, id: u64, side: Side, px: i64, qty: i64, tif: TimeInForce) -> NewOrder {
        NewOrder { owner, id, side, price: Price(px), qty: Qty(qty), tif }
    }

    #[test]
    fn price_time_priority_and_partial_fills() {
        let mut m = MatchingEngine::new();
        let mut ev = Vec::new();
        m.submit(order(1, 1, Side::Sell, 101, 5, TimeInForce::Gtc), &mut ev);
        m.submit(order(2, 2, Side::Sell, 101, 5, TimeInForce::Gtc), &mut ev);
        m.submit(order(3, 3, Side::Sell, 100, 2, TimeInForce::Gtc), &mut ev);
        ev.clear();
        // Buy 8 @ 101: takes 2 @100 (better price first), then 5 from id 1 (earlier), then 1 from id 2.
        m.submit(order(9, 9, Side::Buy, 101, 8, TimeInForce::Gtc), &mut ev);
        let maker_fills: Vec<(u64, i64, i64)> = ev
            .iter()
            .filter_map(|e| match e {
                MatchEvent::Fill { id, qty, price, liquidity: Liquidity::Maker, .. } => Some((*id, price.0, qty.0)),
                _ => None,
            })
            .collect();
        assert_eq!(maker_fills, vec![(3, 100, 2), (1, 101, 5), (2, 101, 1)]);
        assert!(!m.contains(9), "fully filled taker does not rest");
        assert_eq!(m.best_ask(), Some(Price(101)));
        assert!(ev.contains(&MatchEvent::Level { side: Side::Sell, price: Price(101), qty: Qty(4) }));
    }

    #[test]
    fn post_only_ioc_and_cancel() {
        let mut m = MatchingEngine::new();
        let mut ev = Vec::new();
        m.submit(order(1, 1, Side::Sell, 101, 5, TimeInForce::Gtc), &mut ev);
        ev.clear();
        m.submit(order(2, 2, Side::Buy, 101, 1, TimeInForce::PostOnly), &mut ev);
        assert_eq!(ev, vec![MatchEvent::Rejected { owner: 2, id: 2, reason: RejectReason::WouldCross }]);
        ev.clear();
        m.submit(order(2, 3, Side::Buy, 102, 7, TimeInForce::Ioc), &mut ev);
        assert!(ev.contains(&MatchEvent::Canceled { owner: 2, id: 3 }), "IOC remainder cancelled");
        assert_eq!(m.best_ask(), None);
        ev.clear();
        m.submit(order(2, 4, Side::Buy, 99, 3, TimeInForce::Gtc), &mut ev);
        ev.clear();
        m.cancel(5, 4, &mut ev);
        assert!(matches!(ev[0], MatchEvent::Rejected { reason: RejectReason::UnknownOrder, .. }), "wrong owner");
        ev.clear();
        m.cancel(2, 4, &mut ev);
        assert_eq!(
            ev,
            vec![
                MatchEvent::Level { side: Side::Buy, price: Price(99), qty: Qty(0) },
                MatchEvent::Canceled { owner: 2, id: 4 }
            ]
        );
        assert_eq!(m.best_bid(), None);
    }
}
