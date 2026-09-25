//! Messages that flow between threads. All `Copy`, no heap, fixed size.

use crate::types::*;

pub mod md_flags {
    /// Last message of a venue packet/batch: the book is consistent, strategies may act.
    pub const LAST_IN_BATCH: u8 = 1;
    /// Message belongs to a full snapshot rather than an incremental update.
    pub const SNAPSHOT: u8 = 2;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MdKind {
    /// Absolute aggregate quantity at a price level. `qty == 0` deletes the level.
    Level { side: Side, price: Price, qty: Qty },
    /// Public trade print.
    Trade { aggressor: Side, price: Price, qty: Qty },
    /// Real-time best level on one side (e.g. Binance `bookTicker`): sets the
    /// level and removes any better levels on that side that are now stale.
    Top { side: Side, price: Price, qty: Qty },
    /// Drop the whole book (precedes a snapshot).
    Clear,
}

/// Normalised market-data message. Every venue adapter produces exactly this.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MdMsg {
    pub seq: u64,
    /// Venue timestamp (ns, venue clock).
    pub exch_ts: u64,
    /// Local receive timestamp (ns, monotonic `Clock`). Also the replay clock.
    pub recv_ts: u64,
    pub instrument: InstrumentId,
    pub flags: u8,
    pub kind: MdKind,
}

impl MdMsg {
    #[inline(always)]
    pub fn is_last_in_batch(&self) -> bool {
        self.flags & md_flags::LAST_IN_BATCH != 0
    }
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimeInForce {
    Gtc = 0,
    Ioc = 1,
    /// Rejected instead of taking liquidity.
    PostOnly = 2,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OrderCmd {
    New { cl_id: ClOrdId, side: Side, price: Price, qty: Qty, tif: TimeInForce },
    Cancel { cl_id: ClOrdId },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OrderMsg {
    pub ts: u64,
    pub instrument: InstrumentId,
    pub cmd: OrderCmd,
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum RejectReason {
    UnknownOrder = 1,
    WouldCross = 2,
    InvalidOrder = 3,
    RiskMaxQty = 10,
    RiskMaxPosition = 11,
    RiskPriceBand = 12,
    RiskRateLimit = 13,
    RiskMaxOpenOrders = 14,
    RiskKillSwitch = 15,
    RiskNoReference = 16,
}

impl RejectReason {
    pub fn from_u8(v: u8) -> Option<Self> {
        use RejectReason::*;
        Some(match v {
            1 => UnknownOrder,
            2 => WouldCross,
            3 => InvalidOrder,
            10 => RiskMaxQty,
            11 => RiskMaxPosition,
            12 => RiskPriceBand,
            13 => RiskRateLimit,
            14 => RiskMaxOpenOrders,
            15 => RiskKillSwitch,
            16 => RiskNoReference,
            _ => return None,
        })
    }
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Liquidity {
    Maker = 0,
    Taker = 1,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExecKind {
    Ack,
    Fill {
        price: Price,
        qty: Qty,
        leaves: Qty,
        liquidity: Liquidity,
    },
    Canceled,
    /// For a new order: order rejected. For a cancel: cancel rejected (order already gone).
    Rejected {
        reason: RejectReason,
    },
}

/// Execution report from a venue (real, simulated or paper).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ExecMsg {
    pub ts: u64,
    pub instrument: InstrumentId,
    pub cl_id: ClOrdId,
    pub kind: ExecKind,
}
