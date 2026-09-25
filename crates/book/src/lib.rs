//! Price-level (L2) order book.
//!
//! Each side is a contiguous `Vec<Level>` sorted so that the **best price is at the
//! end**. Almost all market-data updates hit the top few levels, so an update is a
//! short backwards scan plus an insert/remove near the end of the vector: no
//! pointer chasing (unlike a tree) and almost no memmove. For deep books the scan
//! falls back to binary search.

use tickrail_core::{MdKind, Price, Qty, Side};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Level {
    pub price: Price,
    pub qty: Qty,
}

const TOP_SCAN: usize = 8;

#[derive(Clone, Debug)]
struct Ladder {
    side: Side,
    /// Ascending by `key`; best level last.
    levels: Vec<Level>,
}

impl Ladder {
    fn new(side: Side, cap: usize) -> Self {
        Ladder { side, levels: Vec::with_capacity(cap) }
    }

    /// Bids: higher price is better; asks: lower is better. Mapping asks to `-price`
    /// lets one ascending-order implementation serve both sides.
    #[inline(always)]
    fn key(side: Side, p: Price) -> i64 {
        match side {
            Side::Buy => p.0,
            Side::Sell => -p.0,
        }
    }

    #[inline]
    fn find(&self, price: Price) -> Result<usize, usize> {
        let k = Self::key(self.side, price);
        let n = self.levels.len();
        let lim = n.saturating_sub(TOP_SCAN);
        let mut i = n;
        while i > lim {
            let lk = Self::key(self.side, self.levels[i - 1].price);
            if lk == k {
                return Ok(i - 1);
            }
            if lk < k {
                return Err(i);
            }
            i -= 1;
        }
        let side = self.side;
        self.levels[..lim].binary_search_by(|l| Self::key(side, l.price).cmp(&k))
    }

    #[inline]
    fn set(&mut self, price: Price, qty: Qty) {
        match self.find(price) {
            Ok(i) if qty.0 <= 0 => {
                self.levels.remove(i);
            }
            Ok(i) => self.levels[i].qty = qty,
            Err(i) if qty.0 > 0 => self.levels.insert(i, Level { price, qty }),
            Err(_) => {}
        }
    }

    #[inline(always)]
    fn best(&self) -> Option<Level> {
        self.levels.last().copied()
    }

    fn iter_from_best(&self) -> impl Iterator<Item = &Level> {
        self.levels.iter().rev()
    }
}

#[derive(Clone, Debug)]
pub struct L2Book {
    bids: Ladder,
    asks: Ladder,
    pub last_trade: Option<(Price, Qty, Side)>,
    pub updates: u64,
}

impl Default for L2Book {
    fn default() -> Self {
        Self::with_capacity(256)
    }
}

impl L2Book {
    pub fn with_capacity(levels_per_side: usize) -> Self {
        L2Book {
            bids: Ladder::new(Side::Buy, levels_per_side),
            asks: Ladder::new(Side::Sell, levels_per_side),
            last_trade: None,
            updates: 0,
        }
    }

    #[inline]
    pub fn apply(&mut self, md: &MdKind) {
        match *md {
            MdKind::Level { side, price, qty } => self.set_level(side, price, qty),
            MdKind::Trade { aggressor, price, qty } => self.last_trade = Some((price, qty, aggressor)),
            MdKind::Top { side, price, qty } => self.set_top(side, price, qty),
            MdKind::Clear => self.clear(),
        }
        self.updates += 1;
    }

    #[inline]
    pub fn set_level(&mut self, side: Side, price: Price, qty: Qty) {
        match side {
            Side::Buy => self.bids.set(price, qty),
            Side::Sell => self.asks.set(price, qty),
        }
    }

    /// Makes `price` the best level on `side`: drops better (stale) levels on that
    /// side and any opposite levels it would cross.
    pub fn set_top(&mut self, side: Side, price: Price, qty: Qty) {
        let (own, opp) = match side {
            Side::Buy => (&mut self.bids, &mut self.asks),
            Side::Sell => (&mut self.asks, &mut self.bids),
        };
        let k = Ladder::key(side, price);
        while own.levels.last().is_some_and(|l| Ladder::key(side, l.price) > k) {
            own.levels.pop();
        }
        // Opposite side is crossed if its best price is at or through ours.
        while opp.levels.last().is_some_and(|l| match side {
            Side::Buy => l.price <= price,
            Side::Sell => l.price >= price,
        }) {
            opp.levels.pop();
        }
        own.set(price, qty);
    }

    pub fn clear(&mut self) {
        self.bids.levels.clear();
        self.asks.levels.clear();
    }

    #[inline(always)]
    pub fn best_bid(&self) -> Option<Level> {
        self.bids.best()
    }

    #[inline(always)]
    pub fn best_ask(&self) -> Option<Level> {
        self.asks.best()
    }

    #[inline(always)]
    pub fn best(&self, side: Side) -> Option<Level> {
        match side {
            Side::Buy => self.bids.best(),
            Side::Sell => self.asks.best(),
        }
    }

    pub fn levels(&self, side: Side) -> impl Iterator<Item = &Level> {
        match side {
            Side::Buy => self.bids.iter_from_best(),
            Side::Sell => self.asks.iter_from_best(),
        }
    }

    pub fn depth(&self, side: Side) -> usize {
        match side {
            Side::Buy => self.bids.levels.len(),
            Side::Sell => self.asks.levels.len(),
        }
    }

    pub fn is_crossed(&self) -> bool {
        matches!((self.best_bid(), self.best_ask()), (Some(b), Some(a)) if b.price >= a.price)
    }

    pub fn spread_ticks(&self) -> Option<i64> {
        Some(self.best_ask()?.price.0 - self.best_bid()?.price.0)
    }

    /// Mid price in ticks (may be fractional).
    pub fn mid(&self) -> Option<f64> {
        Some((self.best_ask()?.price.0 + self.best_bid()?.price.0) as f64 * 0.5)
    }

    /// Size-weighted mid: leans toward the side with less resting size,
    /// i.e. toward where the price is more likely to move next.
    pub fn microprice(&self) -> Option<f64> {
        let (b, a) = (self.best_bid()?, self.best_ask()?);
        let (bq, aq) = (b.qty.0 as f64, a.qty.0 as f64);
        if bq + aq <= 0.0 {
            return self.mid();
        }
        Some((b.price.0 as f64 * aq + a.price.0 as f64 * bq) / (bq + aq))
    }

    /// Volume imbalance over the top `n` levels in [-1, 1]; positive = more bids.
    pub fn imbalance(&self, n: usize) -> f64 {
        let b: i64 = self.bids.iter_from_best().take(n).map(|l| l.qty.0).sum();
        let a: i64 = self.asks.iter_from_best().take(n).map(|l| l.qty.0).sum();
        if a + b == 0 { 0.0 } else { (b - a) as f64 / (b + a) as f64 }
    }

    /// Copies the top `out.len()` levels of one side as (price, qty); returns count.
    pub fn top_n(&self, side: Side, out: &mut [(i64, i64)]) -> usize {
        let mut n = 0;
        for (slot, l) in out.iter_mut().zip(self.levels(side)) {
            *slot = (l.price.0, l.qty.0);
            n += 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lvl(book: &mut L2Book, side: Side, p: i64, q: i64) {
        book.set_level(side, Price(p), Qty(q));
    }

    #[test]
    fn maintains_best_prices() {
        let mut b = L2Book::default();
        lvl(&mut b, Side::Buy, 100, 5);
        lvl(&mut b, Side::Buy, 101, 3);
        lvl(&mut b, Side::Buy, 99, 7);
        lvl(&mut b, Side::Sell, 103, 2);
        lvl(&mut b, Side::Sell, 102, 4);
        lvl(&mut b, Side::Sell, 105, 1);
        assert_eq!(b.best_bid().unwrap().price, Price(101));
        assert_eq!(b.best_ask().unwrap().price, Price(102));
        assert_eq!(b.spread_ticks(), Some(1));
        let bids: Vec<i64> = b.levels(Side::Buy).map(|l| l.price.0).collect();
        assert_eq!(bids, vec![101, 100, 99]);
        let asks: Vec<i64> = b.levels(Side::Sell).map(|l| l.price.0).collect();
        assert_eq!(asks, vec![102, 103, 105]);

        lvl(&mut b, Side::Buy, 101, 0);
        assert_eq!(b.best_bid().unwrap().price, Price(100));
        lvl(&mut b, Side::Sell, 102, 9);
        assert_eq!(b.best_ask().unwrap().qty, Qty(9));
        lvl(&mut b, Side::Sell, 104, 0); // deleting a missing level is a no-op
        assert_eq!(b.depth(Side::Sell), 3);
    }

    #[test]
    fn deep_book_uses_binary_search_path() {
        let mut b = L2Book::default();
        for p in 0..100 {
            lvl(&mut b, Side::Buy, 1000 + p, 1);
            lvl(&mut b, Side::Sell, 2000 + p, 1);
        }
        lvl(&mut b, Side::Buy, 1010, 42);
        lvl(&mut b, Side::Sell, 2090, 0);
        lvl(&mut b, Side::Sell, 2050, 0);
        assert_eq!(b.depth(Side::Sell), 98);
        assert_eq!(b.levels(Side::Buy).find(|l| l.price.0 == 1010).unwrap().qty, Qty(42));
        let asks: Vec<i64> = b.levels(Side::Sell).map(|l| l.price.0).collect();
        assert!(asks.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn top_of_book_removes_stale_levels() {
        let mut b = L2Book::default();
        lvl(&mut b, Side::Buy, 100, 1);
        lvl(&mut b, Side::Buy, 99, 1);
        lvl(&mut b, Side::Sell, 102, 1);
        lvl(&mut b, Side::Sell, 103, 1);
        // Best bid falls to 99: level 100 is stale and must go.
        b.set_top(Side::Buy, Price(99), Qty(5));
        assert_eq!(b.best_bid(), Some(Level { price: Price(99), qty: Qty(5) }));
        // Best ask moves down to 101 (inside the spread).
        b.set_top(Side::Sell, Price(101), Qty(2));
        assert_eq!(b.best_ask().unwrap().price, Price(101));
        assert_eq!(b.depth(Side::Sell), 3);
        // A bid at 102 crosses asks 101 and 102.
        b.set_top(Side::Buy, Price(102), Qty(1));
        assert_eq!(b.best_ask().unwrap().price, Price(103));
        assert!(!b.is_crossed());
    }

    #[test]
    fn microprice_leans_to_thin_side() {
        let mut b = L2Book::default();
        lvl(&mut b, Side::Buy, 100, 9);
        lvl(&mut b, Side::Sell, 102, 1);
        // Tiny ask size → price likely to tick up → microprice near the ask.
        assert!(b.microprice().unwrap() > 101.5);
        assert!(b.imbalance(1) > 0.7);
    }
}
