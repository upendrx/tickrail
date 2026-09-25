//! Replays market data from a CSV file. This is the universal adapter: any
//! market (futures, options, FX, a venue without an adapter yet) can be fed in
//! once its data is converted to this format.
//!
//! ```text
//! ts_ns,event,side,price,qty
//! 1714000000000000000,clear,,,
//! 1714000000000000000,level,B,101.25,300
//! 1714000000000000000,level,S,101.26,120
//! 1714000000150000000,trade,S,101.25,50
//! 1714000000200000000,top,B,101.24,800
//! ```
//!
//! `event` is one of `level`, `top`, `trade`, `clear`. `side` is `B`/`S` (or
//! `buy`/`sell`); for trades it is the aggressor. Rows with the same `ts_ns`
//! form one batch, so the strategy only runs once they have all been applied.
//! `tickrail export --format feed` writes this format from a journal.

use crate::emit::Emitter;
use crate::{AdapterInfo, FeedAdapter, FeedContext};
use anyhow::Context;
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;
use tickrail_core::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub path: PathBuf,
    pub symbol: String,
    pub price_decimals: u32,
    pub qty_decimals: u32,
    /// 1.0 = original pace, 10.0 = ten times faster, 0 = as fast as possible.
    #[serde(default = "default_speed")]
    pub speed: f64,
}

fn default_speed() -> f64 {
    1.0
}

pub struct CsvFeed {
    cfg: Config,
    inst: Instrument,
}

pub fn create(cfg: Config, instrument_id: u32) -> anyhow::Result<Box<dyn FeedAdapter>> {
    anyhow::ensure!(cfg.path.exists(), "CSV file {} not found", cfg.path.display());
    let inst = Instrument {
        id: instrument_id,
        symbol: cfg.symbol.clone(),
        price_decimals: cfg.price_decimals,
        qty_decimals: cfg.qty_decimals,
    };
    Ok(Box::new(CsvFeed { cfg, inst }))
}

/// Parses one row into (timestamp, event). Header, blank and `#` lines give `None`.
pub fn parse_row(line: &str, inst: &Instrument) -> Result<Option<(u64, MdKind)>, String> {
    let mut f = line.split(',').map(str::trim);
    let ts = f.next().unwrap_or("");
    if ts.is_empty() || ts.starts_with('#') || ts == "ts_ns" {
        return Ok(None);
    }
    let ts: u64 = ts.parse().map_err(|_| format!("bad ts_ns `{ts}`"))?;
    let event = f.next().unwrap_or("");
    let side = match f.next().unwrap_or("") {
        "B" | "b" | "buy" | "BUY" | "bid" => Some(Side::Buy),
        "S" | "s" | "sell" | "SELL" | "ask" => Some(Side::Sell),
        _ => None,
    };
    let price = f.next().unwrap_or("");
    let qty = f.next().unwrap_or("");
    let need = |x: Option<Side>| x.ok_or_else(|| format!("{event} row needs a side"));
    let px =
        || parse_fixed(price.as_bytes(), inst.price_decimals).map(Price).ok_or_else(|| format!("bad price `{price}`"));
    let q = || parse_fixed(qty.as_bytes(), inst.qty_decimals).map(Qty).ok_or_else(|| format!("bad qty `{qty}`"));
    let kind = match event {
        "clear" => MdKind::Clear,
        "level" => MdKind::Level { side: need(side)?, price: px()?, qty: q()? },
        "top" => MdKind::Top { side: need(side)?, price: px()?, qty: q()? },
        "trade" => MdKind::Trade { aggressor: need(side)?, price: px()?, qty: q()? },
        other => return Err(format!("unknown event `{other}`")),
    };
    Ok(Some((ts, kind)))
}

/// Loads a whole file for offline replay. Timestamps become both venue and
/// receive time, and rows sharing a timestamp are closed as one batch.
pub fn read_file(path: &std::path::Path, inst: &Instrument) -> anyhow::Result<Vec<MdMsg>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut msgs: Vec<MdMsg> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        match parse_row(line, inst) {
            Ok(Some((ts, kind))) => {
                if let Some(last) = msgs.last_mut()
                    && last.exch_ts != ts
                {
                    last.flags |= md_flags::LAST_IN_BATCH;
                }
                let seq = msgs.len() as u64 + 1;
                msgs.push(MdMsg { seq, exch_ts: ts, recv_ts: ts, instrument: inst.id, flags: 0, kind });
            }
            Ok(None) => {}
            Err(e) => anyhow::bail!("{} line {}: {e}", path.display(), n + 1),
        }
    }
    if let Some(last) = msgs.last_mut() {
        last.flags |= md_flags::LAST_IN_BATCH;
    }
    Ok(msgs)
}

impl FeedAdapter for CsvFeed {
    fn info(&self) -> AdapterInfo {
        AdapterInfo {
            adapter: "csv".into(),
            venue: format!("file {}", self.cfg.path.display()),
            asset_class: "file".into(),
            endpoint: self.cfg.path.display().to_string(),
            streams: vec![("ts_ns,event,side,price,qty".into(), format!("Replayed at {}x speed", self.cfg.speed))],
            needs_credentials: false,
        }
    }

    fn instrument(&self) -> &Instrument {
        &self.inst
    }

    fn start(self: Box<Self>, ctx: FeedContext) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("feed-csv".into())
            .spawn(move || {
                if let Err(e) = run(*self, ctx) {
                    eprintln!("[feed] csv: {e:#}");
                }
            })
            .expect("spawn csv feed")
    }
}

fn run(feed: CsvFeed, ctx: FeedContext) -> anyhow::Result<()> {
    let FeedContext { clock, tx, stop, stats } = ctx;
    let file = std::fs::File::open(&feed.cfg.path).with_context(|| format!("open {}", feed.cfg.path.display()))?;
    let mut out = Emitter::new(feed.inst.clone(), tx, stats.clone());
    stats.connected.store(true, Ordering::Relaxed);
    let speed = feed.cfg.speed;
    let (mut first_ts, start) = (None::<u64>, clock.now());
    let mut batch_ts = None;
    for (n, line) in BufReader::new(file).lines().enumerate() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let line = line?;
        let row = match parse_row(&line, &feed.inst) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                stats.parse_errors.fetch_add(1, Ordering::Relaxed);
                eprintln!("[feed] csv line {}: {e}", n + 1);
                continue;
            }
        };
        if batch_ts.is_some_and(|t| t != row.0) {
            out.flush();
        }
        if batch_ts != Some(row.0) && speed > 0.0 {
            let t0 = *first_ts.get_or_insert(row.0);
            let due = start + ((row.0.saturating_sub(t0)) as f64 / speed) as u64;
            while clock.now() < due && !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_micros(200));
            }
        }
        batch_ts = Some(row.0);
        out.recv_ts = clock.now();
        stats.frames.fetch_add(1, Ordering::Relaxed);
        out.push(row.1, 0, row.0);
    }
    out.flush();
    stats.connected.store(false, Ordering::Relaxed);
    eprintln!("[feed] csv: end of file");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rows() {
        let out = Instrument { id: 1, symbol: "ES".into(), price_decimals: 2, qty_decimals: 0 };
        assert_eq!(parse_row("ts_ns,event,side,price,qty", &out), Ok(None));
        assert_eq!(
            parse_row("5,level,B,101.25,300", &out),
            Ok(Some((5, MdKind::Level { side: Side::Buy, price: Price(10_125), qty: Qty(300) })))
        );
        assert_eq!(
            parse_row("6,trade,sell,101.25,5", &out),
            Ok(Some((6, MdKind::Trade { aggressor: Side::Sell, price: Price(10_125), qty: Qty(5) })))
        );
        assert_eq!(parse_row("7,clear,,,", &out), Ok(Some((7, MdKind::Clear))));
        assert!(parse_row("8,level,,1,1", &out).is_err());
        assert!(parse_row("9,level,B,1.234,1", &out).is_err());
    }
}
