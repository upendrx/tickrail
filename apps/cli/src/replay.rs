//! Deterministic backtest: feeds recorded market data through the engine with
//! paper fills, using the `[strategy]`, `[risk]` and `[execution]` sections of a
//! config.
//!
//! Input is either a journal (instrument read from its header) or a CSV file in
//! the `csv` adapter's format (instrument from `[feed]`: symbol, price_decimals,
//! qty_decimals). CSV timestamps become event time, so time-based strategy logic
//! behaves as it would have live.

use crate::config::Config;
use crate::run::risk_limits;
use crate::summary;
use anyhow::{Context, Result};
use hdrhistogram::Histogram;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use tickrail_core::ring::{self, Consumer};
use tickrail_core::*;
use tickrail_engine::stats::{LatKind, LatSample};
use tickrail_engine::{Engine, EngineOptions, Outputs, PaperVenue};
use tickrail_journal::{JournalReader, Record};
use tickrail_oms::{FeeSchedule, Oms};
use tickrail_risk::RiskEngine;

/// Market data from either input format, in file order.
fn load_events(path: &Path, cfg: &Config) -> Result<(Instrument, Box<dyn Iterator<Item = MdMsg>>)> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("csv")) {
        let get =
            |k: &str| cfg.feed.get(k).with_context(|| format!("CSV replay needs feed.{k} (e.g. --set feed.{k}=...)"));
        let inst = Instrument {
            id: 1,
            symbol: get("symbol")?.as_str().unwrap_or("CSV").to_string(),
            price_decimals: get("price_decimals")?.as_integer().context("feed.price_decimals must be an integer")?
                as u32,
            qty_decimals: get("qty_decimals")?.as_integer().context("feed.qty_decimals must be an integer")? as u32,
        };
        let msgs = tickrail_adapters::csv::read_file(path, &inst)?;
        return Ok((inst, Box::new(msgs.into_iter())));
    }
    let reader = JournalReader::open(path)?;
    let inst = reader.instrument.clone();
    Ok((inst, Box::new(reader.filter_map(|r| if let Record::Md(m) = r { Some(m) } else { None }))))
}

pub fn run(path: &Path, cfg: Config) -> Result<()> {
    let (inst, events) = load_events(path, &cfg)?;
    let name = cfg.strategy.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let mut params = cfg.strategy.clone();
    params.remove("name");
    let (strategy, _) = tickrail_strategies::registry::build(&name, params, &inst).map_err(anyhow::Error::msg)?;
    let (lat_tx, mut lat_rx) = ring::channel::<LatSample>(1 << 16);
    let mut engine = Engine::new(
        inst.clone(),
        Clock::new(),
        strategy,
        PaperVenue::new(inst.id),
        Oms::new(&inst, FeeSchedule { maker_bps: cfg.execution.maker_fee_bps, taker_bps: cfg.execution.taker_fee_bps }),
        RiskEngine::new(risk_limits(&cfg.risk, &inst), Arc::new(AtomicBool::new(false))),
        Outputs { journal: None, latency: Some(lat_tx), stats: None },
        EngineOptions { replay: true, ..EngineOptions::default() },
    );
    let mut hist = Histogram::<u64>::new_with_bounds(1, 1_000_000_000, 3)?;
    let mut drain = |rx: &mut Consumer<LatSample>| {
        while let Some(s) = rx.pop() {
            if s.kind == LatKind::TickToOrder {
                let _ = hist.record(s.ns.max(1));
            }
        }
    };
    let started = Instant::now();
    let mut n = 0u64;
    for md in events {
        engine.on_md(&md);
        n += 1;
        if n.is_multiple_of(4096) {
            drain(&mut lat_rx);
            engine.housekeeping();
        }
    }
    drain(&mut lat_rx);
    let secs = started.elapsed().as_secs_f64();
    let stats = engine.shutdown();
    println!("replay {} ({} {})", path.display(), inst.symbol, name);
    println!(
        "  {n} market-data events in {secs:.3}s: {:.2} M events/s, {:.0} ns/event",
        n as f64 / secs / 1e6,
        secs * 1e9 / n.max(1) as f64
    );
    if !hist.is_empty() {
        println!(
            "  decision latency p50 {} ns  p99 {} ns  p99.9 {} ns  max {} ns",
            hist.value_at_quantile(0.5),
            hist.value_at_quantile(0.99),
            hist.value_at_quantile(0.999),
            hist.max()
        );
    }
    summary::print(&inst, &stats, None, None);
    Ok(())
}
