//! Builds strategies by name from their config section.
//!
//! Sizes in config files are in instrument units (e.g. 0.001 BTC, 10 shares)
//! and converted to lots here, so the same config reads naturally for any market.
//! To add a strategy: implement [`Strategy`](crate::Strategy), add a params
//! struct below, and add one arm to [`build`].

use crate::{AnyStrategy, ImbalanceParams, ImbalanceTaker, MarketMaker, MarketMakerParams};
use serde::Deserialize;
use tickrail_core::{Instrument, Qty};

/// (name, one-line description) of every built-in strategy.
pub const BUILTIN: &[(&str, &str)] = &[
    ("market_maker", "Inventory-skewed two-sided quoting (Avellaneda-Stoikov style)"),
    ("imbalance", "Crosses the spread when the top of book is heavily one-sided"),
];

/// A parameter as shown to users: (name, value, what it does).
pub type ParamDoc = (String, String, String);

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketMakerConfig {
    /// Quote size in instrument units.
    pub size: f64,
    /// Largest position the strategy will build, in instrument units.
    pub max_inventory: f64,
    #[serde(default = "d_half_spread")]
    pub half_spread_ticks: f64,
    #[serde(default = "d_vol_mult")]
    pub vol_mult: f64,
    #[serde(default = "d_skew")]
    pub skew_ticks: f64,
    #[serde(default = "d_requote")]
    pub requote_ticks: i64,
    #[serde(default = "d_min_requote_us")]
    pub min_requote_us: u64,
}

fn d_half_spread() -> f64 {
    2.0
}
fn d_vol_mult() -> f64 {
    1.5
}
fn d_skew() -> f64 {
    10.0
}
fn d_requote() -> i64 {
    2
}
fn d_min_requote_us() -> u64 {
    10_000
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImbalanceConfig {
    pub size: f64,
    pub max_inventory: f64,
    #[serde(default = "d_threshold")]
    pub threshold: f64,
    #[serde(default = "d_levels")]
    pub levels: usize,
    #[serde(default = "d_cooldown_ms")]
    pub cooldown_ms: u64,
}

fn d_threshold() -> f64 {
    0.6
}
fn d_levels() -> usize {
    3
}
fn d_cooldown_ms() -> u64 {
    50
}

fn lots(inst: &Instrument, units: f64) -> Qty {
    inst.to_lots(units).max(Qty(1))
}

/// Builds a built-in strategy. `params` is the `[strategy]` table minus `name`.
pub fn build(name: &str, params: toml::Table, inst: &Instrument) -> Result<(AnyStrategy, Vec<ParamDoc>), String> {
    let value = toml::Value::Table(params);
    let sym = &inst.symbol;
    match name {
        "market_maker" => {
            let c: MarketMakerConfig = value.try_into().map_err(|e| format!("[strategy] market_maker: {e}"))?;
            let size = lots(inst, c.size);
            let docs = vec![
                p("size", format!("{} {sym}", c.size), "Quantity of every quote."),
                p(
                    "max_inventory",
                    format!("{} {sym}", c.max_inventory),
                    "Stop quoting the side that would push the position past this.",
                ),
                p(
                    "half_spread_ticks",
                    c.half_spread_ticks,
                    "Minimum distance between a quote and the reservation price.",
                ),
                p(
                    "vol_mult",
                    c.vol_mult,
                    "Extra half-spread per unit of recent volatility, so quotes widen in fast markets.",
                ),
                p("skew_ticks", c.skew_ticks, "How far quotes lean away from the position when inventory is full."),
                p(
                    "requote_ticks",
                    c.requote_ticks,
                    "Only replace a quote when its target moved this far. Keeps queue priority.",
                ),
                p("min_requote_us", c.min_requote_us, "Minimum time between new quotes on one side."),
            ];
            let mm = MarketMaker::new(MarketMakerParams {
                quote_qty: size,
                max_inventory: lots(inst, c.max_inventory).0.max(size.0),
                base_half_spread_ticks: c.half_spread_ticks,
                vol_mult: c.vol_mult,
                skew_ticks_at_max: c.skew_ticks,
                requote_ticks: c.requote_ticks.max(1),
                min_requote_ns: c.min_requote_us * 1_000,
            });
            Ok((AnyStrategy::MarketMaker(mm), docs))
        }
        "imbalance" => {
            let c: ImbalanceConfig = value.try_into().map_err(|e| format!("[strategy] imbalance: {e}"))?;
            let size = lots(inst, c.size);
            let docs = vec![
                p("size", format!("{} {sym}", c.size), "Quantity of every order."),
                p("max_inventory", format!("{} {sym}", c.max_inventory), "Largest position the strategy will build."),
                p("threshold", c.threshold, "Cross the spread when (bid size - ask size) / total exceeds this."),
                p("levels", c.levels, "How many levels per side the imbalance is measured over."),
                p("cooldown_ms", c.cooldown_ms, "Minimum time between orders."),
            ];
            let s = ImbalanceTaker::new(ImbalanceParams {
                levels: c.levels.max(1),
                threshold: c.threshold,
                qty: size,
                max_inventory: lots(inst, c.max_inventory).0.max(size.0),
                cooldown_ns: c.cooldown_ms * 1_000_000,
            });
            Ok((AnyStrategy::Imbalance(s), docs))
        }
        other => {
            let known: Vec<&str> = BUILTIN.iter().map(|b| b.0).collect();
            Err(format!("unknown strategy `{other}` (built-in: {})", known.join(", ")))
        }
    }
}

fn p(name: &str, value: impl ToString, what: &str) -> ParamDoc {
    (name.to_string(), value.to_string(), what.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst() -> Instrument {
        Instrument { id: 1, symbol: "ETHUSDT".into(), price_decimals: 2, qty_decimals: 4 }
    }

    #[test]
    fn builds_from_toml() {
        let t: toml::Table = toml::from_str("size = 0.01\nmax_inventory = 0.1\nskew_ticks = 20").unwrap();
        let (s, docs) = build("market_maker", t, &inst()).unwrap();
        assert!(matches!(s, AnyStrategy::MarketMaker(_)));
        assert_eq!(docs[0].1, "0.01 ETHUSDT");
    }

    #[test]
    fn rejects_typos_and_unknown_names() {
        let t: toml::Table = toml::from_str("size = 1\nmax_inventory = 2\nskwe_ticks = 3").unwrap();
        let err = build("market_maker", t, &inst()).err().unwrap();
        assert!(err.contains("skwe_ticks"), "{err}");
        assert!(build("nope", toml::Table::new(), &inst()).err().unwrap().contains("market_maker"));
    }
}
