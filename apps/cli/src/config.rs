//! The config file. One TOML document picks every component by name:
//!
//! ```toml
//! [feed]        adapter = "binance"   # or okx, kraken, alpaca, csv, simulator
//! [execution]   venue = "paper"       # or simulator
//! [strategy]    name = "market_maker"
//! [risk]        ...                   # hard limits, always explicit
//! [engine]      wait = "spin"         # spin, yield, backoff, sleep:200us
//! [http]        listen = "127.0.0.1:8080"
//! ```
//!
//! Any value can be overridden on the command line: `--set feed.symbol=ETHUSDT`.

use anyhow::{Context, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub engine: EngineSection,
    /// Adapter name plus adapter-specific keys, passed through untouched.
    pub feed: toml::Table,
    #[serde(default)]
    pub execution: ExecutionSection,
    /// Strategy name plus strategy-specific keys.
    pub strategy: toml::Table,
    pub risk: RiskSection,
    #[serde(default)]
    pub http: HttpSection,
    /// Only read when `feed.adapter = "simulator"`.
    #[serde(default)]
    pub simulator: SimulatorSection,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EngineSection {
    /// What the engine thread does when idle.
    pub wait: String,
    /// Pin the engine thread to this core (Linux).
    pub core: Option<usize>,
    /// Record every event to this file.
    pub journal: Option<PathBuf>,
    /// Stop after this many seconds.
    pub duration_secs: Option<u64>,
    /// Market-data ring capacity (messages).
    pub md_ring: usize,
}

impl Default for EngineSection {
    fn default() -> Self {
        EngineSection { wait: "backoff".into(), core: None, journal: None, duration_secs: None, md_ring: 1 << 16 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ExecutionSection {
    /// "paper" (simulated fills against the real feed) or "simulator".
    pub venue: String,
    /// Negative = rebate.
    pub maker_fee_bps: f64,
    pub taker_fee_bps: f64,
}

impl Default for ExecutionSection {
    fn default() -> Self {
        ExecutionSection { venue: "paper".into(), maker_fee_bps: 0.0, taker_fee_bps: 0.0 }
    }
}

/// Hard limits. Sizes are in instrument units, like the strategy's.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskSection {
    pub max_order_size: f64,
    pub max_position: f64,
    #[serde(default = "d_open")]
    pub max_open_orders: usize,
    /// Reject orders priced further than this from mid.
    pub price_band_ticks: i64,
    #[serde(default = "d_rate")]
    pub max_orders_per_sec: u32,
    /// Kill switch trips when PnL falls below minus this (quote currency).
    pub max_loss: f64,
}

fn d_open() -> usize {
    16
}
fn d_rate() -> u32 {
    200
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HttpSection {
    pub enabled: bool,
    pub listen: String,
    /// Serve the web UI from this folder (edit without recompiling).
    pub ui_dir: Option<PathBuf>,
}

impl Default for HttpSection {
    fn default() -> Self {
        HttpSection { enabled: true, listen: "127.0.0.1:8080".into(), ui_dir: None }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SimulatorSection {
    pub symbol: String,
    pub price_decimals: u32,
    pub qty_decimals: u32,
    pub start_price: f64,
    pub seed: u64,
    /// Background events per second.
    pub event_rate: f64,
    /// One-way order latency, microseconds.
    pub latency_us: u64,
    /// Hidden fair-value volatility, ticks per sqrt(second).
    pub volatility: f64,
    pub core: Option<usize>,
}

impl Default for SimulatorSection {
    fn default() -> Self {
        SimulatorSection {
            symbol: "SIM".into(),
            price_decimals: 2,
            qty_decimals: 3,
            start_price: 60_000.0,
            seed: 42,
            event_rate: 20_000.0,
            latency_us: 20,
            volatility: 150.0,
            core: None,
        }
    }
}

/// Built-in config used when no file is given: the local simulator.
pub const DEFAULT: &str = include_str!("../../../config/simulator.toml");

pub fn load(path: Option<&Path>, overrides: &[String]) -> anyhow::Result<(Config, String)> {
    let (text, origin) = match path {
        Some(p) => {
            (std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?, p.display().to_string())
        }
        None => (DEFAULT.to_string(), "built-in simulator config".to_string()),
    };
    let mut doc: toml::Table = toml::from_str(&text).with_context(|| format!("parsing {origin}"))?;
    for o in overrides {
        apply_override(&mut doc, o)?;
    }
    let cfg: Config = toml::Value::Table(doc).try_into().with_context(|| format!("invalid config ({origin})"))?;
    Ok((cfg, origin))
}

/// `a.b.c=value`. The value is parsed as TOML (numbers, booleans, arrays) and
/// falls back to a plain string, so `--set feed.symbol=ETHUSDT` just works.
fn apply_override(doc: &mut toml::Table, spec: &str) -> anyhow::Result<()> {
    let Some((path, raw)) = spec.split_once('=') else { bail!("--set expects key=value, got `{spec}`") };
    let value = toml::from_str::<toml::Table>(&format!("v = {raw}"))
        .ok()
        .and_then(|mut t| t.remove("v"))
        .unwrap_or_else(|| toml::Value::String(raw.to_string()));
    let keys: Vec<&str> = path.trim().split('.').collect();
    let (last, parents) = keys.split_last().context("empty key")?;
    let mut t = doc;
    for k in parents {
        t = t
            .entry(k.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .with_context(|| format!("`{k}` in `{path}` is not a table"))?;
    }
    t.insert(last.to_string(), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let (c, _) = load(None, &[]).unwrap();
        assert_eq!(c.feed["adapter"].as_str(), Some("simulator"));
    }

    #[test]
    fn overrides_types_and_nesting() {
        let (c, _) =
            load(None, &["engine.wait=spin".into(), "risk.max_loss=5".into(), "simulator.seed=7".into()]).unwrap();
        assert_eq!(c.engine.wait, "spin");
        assert_eq!(c.risk.max_loss, 5.0);
        assert_eq!(c.simulator.seed, 7);
    }

    #[test]
    fn example_configs_parse() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().is_some_and(|e| e == "toml") {
                load(Some(&p), &[]).unwrap_or_else(|e| panic!("{}: {e:#}", p.display()));
            }
        }
    }
}
