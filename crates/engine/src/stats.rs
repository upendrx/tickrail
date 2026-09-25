//! Snapshot types the engine publishes for monitoring. Plain `Copy` data so a
//! snapshot can go through an SPSC ring without allocating.

use serde::Serialize;

pub const BOOK_LEVELS: usize = 20;
pub const TRADE_ROWS: usize = 24;
pub const FILL_ROWS: usize = 16;
pub const ORDER_ROWS: usize = 12;
pub const STRAT_FIELDS: usize = 8;
/// Order of `EngineStats::rejects_by_reason`.
pub const REJECT_REASONS: [&str; 8] = [
    "max order size",
    "max position",
    "price band",
    "rate limit",
    "max open orders",
    "kill switch",
    "no market",
    "other",
];

#[derive(Copy, Clone, Debug, Default, Serialize)]
pub struct TradeRow {
    pub ts_ns: u64,
    pub price: i64,
    pub qty: i64,
    /// 0 = buyer was aggressor, 1 = seller.
    pub side: u8,
}

#[derive(Copy, Clone, Debug, Default, Serialize)]
pub struct FillRow {
    pub ts_ns: u64,
    pub cl_id: u64,
    pub side: u8,
    pub price: i64,
    pub qty: i64,
    pub maker: bool,
    /// Mid (ticks x2) at fill time, for instant markout.
    pub mid_x2: i64,
}

#[derive(Copy, Clone, Debug, Default, Serialize)]
pub struct OrderRow {
    pub cl_id: u64,
    pub side: u8,
    pub price: i64,
    pub qty: i64,
    pub leaves: i64,
    /// 0 pending new, 1 live, 2 pending cancel.
    pub status: u8,
    pub age_ns: u64,
}

#[derive(Copy, Clone, Debug, Default, Serialize)]
pub struct EngineStats {
    pub ts_ns: u64,
    pub md_msgs: u64,
    pub md_gaps: u64,
    pub orders_sent: u64,
    pub cancels_sent: u64,
    pub fills: u64,
    pub maker_fills: u64,
    pub risk_rejects: u64,
    pub venue_rejects: u64,
    pub open_orders: u64,
    pub position_lots: i64,
    pub volume_lots: i64,
    pub pnl: f64,
    pub fees: f64,
    pub best_bid: i64,
    pub best_ask: i64,
    pub our_bid: i64,
    pub our_ask: i64,
    pub bids: [(i64, i64); BOOK_LEVELS],
    pub asks: [(i64, i64); BOOK_LEVELS],
    pub killed: bool,
    pub journal_dropped: u64,
    pub book_batches: u64,
    pub trades_seen: u64,
    pub open_buy_qty: i64,
    pub open_sell_qty: i64,
    pub rejects_by_reason: [u64; 8],
    pub strat: [f64; STRAT_FIELDS],
    pub trades: [TradeRow; TRADE_ROWS],
    pub fills_recent: [FillRow; FILL_ROWS],
    pub orders: [OrderRow; ORDER_ROWS],
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LatKind {
    /// Market-data receive → order handed to the gateway (software tick-to-trade).
    TickToOrder = 0,
    /// Market-data receive → engine starts processing (queueing / ring hop).
    MdQueue = 1,
    /// Order sent → venue ack (round trip, includes simulated wire latency).
    OrderAck = 2,
    /// Whole engine pass for one market-data message (book update + strategy).
    MdProcess = 3,
}

#[derive(Copy, Clone, Debug)]
pub struct LatSample {
    pub kind: LatKind,
    pub ns: u64,
}
