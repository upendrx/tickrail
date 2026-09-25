//! Fixed-point numeric types. Floating point never touches order prices.

/// Price in integer ticks (e.g. 6_000_000 = 60000.00 with a 0.01 tick).
#[repr(transparent)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price(pub i64);

/// Quantity in integer lots (e.g. 100 = 0.001 BTC with a 0.00001 lot).
#[repr(transparent)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Qty(pub i64);

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Buy = 0,
    Sell = 1,
}

impl Side {
    #[inline(always)]
    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }

    /// +1 for buys, -1 for sells.
    #[inline(always)]
    pub fn sign(self) -> i64 {
        match self {
            Side::Buy => 1,
            Side::Sell => -1,
        }
    }

    pub fn from_u8(v: u8) -> Option<Side> {
        match v {
            0 => Some(Side::Buy),
            1 => Some(Side::Sell),
            _ => None,
        }
    }
}

pub type InstrumentId = u32;
pub type ClOrdId = u64;

/// Static instrument definition. Tick = 10^-price_decimals, lot = 10^-qty_decimals.
#[derive(Clone, Debug)]
pub struct Instrument {
    pub id: InstrumentId,
    pub symbol: String,
    pub price_decimals: u32,
    pub qty_decimals: u32,
}

impl Instrument {
    pub fn tick_size(&self) -> f64 {
        10f64.powi(-(self.price_decimals as i32))
    }

    pub fn lot_size(&self) -> f64 {
        10f64.powi(-(self.qty_decimals as i32))
    }

    /// Ticks to a decimal price. Divides by a power of ten rather than
    /// multiplying by the tick size, so 9999 ticks prints as 99.99, not 99.99000000000001.
    pub fn px(&self, p: Price) -> f64 {
        p.0 as f64 / 10f64.powi(self.price_decimals as i32)
    }

    pub fn qty(&self, q: Qty) -> f64 {
        q.0 as f64 / 10f64.powi(self.qty_decimals as i32)
    }

    pub fn to_ticks(&self, px: f64) -> Price {
        Price((px * 10f64.powi(self.price_decimals as i32)).round() as i64)
    }

    pub fn to_lots(&self, q: f64) -> Qty {
        Qty((q * 10f64.powi(self.qty_decimals as i32)).round() as i64)
    }

    /// Value of one (tick x lot) unit in quote currency.
    pub fn notional_unit(&self) -> f64 {
        self.tick_size() * self.lot_size()
    }
}

/// Exact decimal-string -> fixed-point parser ("63123.4500" with 2 decimals -> 6312345).
///
/// Venue feeds send prices as strings; going through `f64` would be both slower and
/// lossy. Extra fractional digits beyond `decimals` must be zeros, otherwise `None`.
pub fn parse_fixed(s: &[u8], decimals: u32) -> Option<i64> {
    let (neg, digits) = match s.first()? {
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    let mut int: i64 = 0;
    let mut frac_digits = 0u32;
    let mut seen_dot = false;
    for &c in digits {
        match c {
            b'0'..=b'9' => {
                let d = (c - b'0') as i64;
                if seen_dot {
                    if frac_digits >= decimals {
                        if d != 0 {
                            return None;
                        }
                        continue;
                    }
                    frac_digits += 1;
                }
                int = int.checked_mul(10)?.checked_add(d)?;
            }
            b'.' if !seen_dot => seen_dot = true,
            _ => return None,
        }
    }
    while frac_digits < decimals {
        int = int.checked_mul(10)?;
        frac_digits += 1;
    }
    Some(if neg { -int } else { int })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixed_point() {
        assert_eq!(parse_fixed(b"63123.45000000", 2), Some(6_312_345));
        assert_eq!(parse_fixed(b"0.00100000", 5), Some(100));
        assert_eq!(parse_fixed(b"12", 3), Some(12_000));
        assert_eq!(parse_fixed(b"-1.5", 1), Some(-15));
        assert_eq!(parse_fixed(b"1.234", 2), None, "sub-tick digits rejected");
        assert_eq!(parse_fixed(b"1.2.3", 2), None);
        assert_eq!(parse_fixed(b"", 2), None);
    }
}
