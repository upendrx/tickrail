//! Wires the configured components together and runs them.
//!
//! ```text
//! feed thread ──MdMsg ring──▶ engine thread ──▶ venue (paper, in-process)
//!                                   │  └──orders ring──▶ simulator thread ──execs ring──┐
//!                                   ├──journal ring──▶ journal thread                   │
//!                                   └──stats/latency rings──▶ telemetry (tokio) ◀───────┘
//! ```

use crate::config::{Config, RiskSection};
use crate::summary;
use anyhow::{Context, Result, bail};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tickrail_adapters::{FeedContext, FeedStats};
use tickrail_core::ring::{self, Consumer};
use tickrail_core::*;
use tickrail_engine::stats::{EngineStats, LatSample};
use tickrail_engine::{Engine, EngineOptions, Outputs, PaperVenue, SimGateway, run_loop};
use tickrail_journal::{JournalWriter, Record};
use tickrail_oms::{FeeSchedule, Oms};
use tickrail_risk::{RiskEngine, RiskLimits};
use tickrail_sim::{VenueConfig, VenueSim};
use tickrail_strategies::Strategy;
use tickrail_telemetry::{FeedProbe, SessionInfo, Snapshot, Telemetry};

pub fn risk_limits(r: &RiskSection, inst: &Instrument) -> RiskLimits {
    RiskLimits {
        max_order_qty: inst.to_lots(r.max_order_size),
        max_position: inst.to_lots(r.max_position).0,
        max_open_orders: r.max_open_orders,
        price_band_ticks: r.price_band_ticks,
        max_orders_per_sec: r.max_orders_per_sec,
        max_loss: r.max_loss,
    }
}

fn is_simulator(cfg: &Config) -> bool {
    cfg.feed.get("adapter").and_then(|v| v.as_str()) == Some("simulator")
}

/// Checks the parts of a config that serde can't: names, combinations, limits.
pub fn validate(cfg: &Config) -> Result<()> {
    cfg.engine.wait.parse::<WaitStrategy>().map_err(anyhow::Error::msg).context("[engine] wait")?;
    let sim_feed = is_simulator(cfg);
    match (sim_feed, cfg.execution.venue.as_str()) {
        (true, "simulator") | (false, "paper") => {}
        (true, v) => bail!("feed.adapter = \"simulator\" needs execution.venue = \"simulator\" (got \"{v}\")"),
        (false, "simulator") => bail!("execution.venue = \"simulator\" only works with feed.adapter = \"simulator\""),
        (false, v) => bail!("unknown execution venue \"{v}\" (available: paper, simulator)"),
    }
    let name = cfg.strategy.get("name").and_then(|v| v.as_str()).context("[strategy] needs name = \"...\"")?;
    let probe = Instrument { id: 1, symbol: "CHECK".into(), price_decimals: 2, qty_decimals: 8 };
    let mut params = cfg.strategy.clone();
    params.remove("name");
    tickrail_strategies::registry::build(name, params, &probe).map_err(anyhow::Error::msg)?;
    let r = &cfg.risk;
    anyhow::ensure!(r.max_order_size > 0.0 && r.max_position > 0.0, "[risk] sizes must be positive");
    anyhow::ensure!(
        r.price_band_ticks > 0 && r.max_loss > 0.0,
        "[risk] price_band_ticks and max_loss must be positive"
    );
    if !sim_feed {
        let adapter = cfg.feed.get("adapter").and_then(|v| v.as_str()).unwrap_or("");
        anyhow::ensure!(
            tickrail_adapters::available().contains(&adapter),
            "unknown feed adapter \"{adapter}\" (available: simulator, {})",
            tickrail_adapters::available().join(", ")
        );
    }
    Ok(())
}

fn spawn_named<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> std::thread::JoinHandle<T> {
    std::thread::Builder::new().name(name.into()).spawn(f).expect("spawn thread")
}

pub fn run(cfg: Config) -> Result<()> {
    validate(&cfg)?;
    let wait: WaitStrategy = cfg.engine.wait.parse().map_err(anyhow::Error::msg)?;
    let clock = Clock::new();
    let stop = Arc::new(AtomicBool::new(false));
    let kill = Arc::new(AtomicBool::new(false));
    let (md_tx, md_rx) = ring::channel::<MdMsg>(cfg.engine.md_ring);
    let sim = is_simulator(&cfg);

    // 1. Market data: an adapter thread, or the simulator (which is also the venue).
    let mut sim_parts = None;
    let mut feed_parts = None;
    let (inst, adapter_info) = if sim {
        let s = &cfg.simulator;
        let inst = Instrument {
            id: 1,
            symbol: s.symbol.clone(),
            price_decimals: s.price_decimals,
            qty_decimals: s.qty_decimals,
        };
        let (ord_tx, ord_rx) = ring::channel::<OrderMsg>(1 << 12);
        let (exec_tx, exec_rx) = ring::channel::<ExecMsg>(1 << 14);
        let vcfg = VenueConfig {
            instrument: inst.id,
            seed: s.seed,
            bg_rate: s.event_rate,
            order_latency_ns: s.latency_us * 1_000,
            start_price: inst.to_ticks(s.start_price),
            vol_ticks_per_sqrt_s: s.volatility,
            wait,
            core: s.core,
            ..VenueConfig::default()
        };
        let venue = VenueSim::new(vcfg, clock, md_tx, exec_tx, ord_rx);
        let stop2 = stop.clone();
        let handle = spawn_named("venue-sim", move || venue.run(stop2));
        sim_parts = Some((handle, SimGateway { tx: ord_tx, rx: exec_rx }));
        let info = (
            "simulator".to_string(),
            "Built-in exchange simulator".to_string(),
            "simulated".to_string(),
            format!("{} background events/s, {} µs order latency, seed {}", s.event_rate, s.latency_us, s.seed),
            vec![(
                "synthetic order flow".to_string(),
                "Limit orders, cancels and aggressive orders around a hidden fair value".to_string(),
            )],
        );
        (inst, info)
    } else {
        let adapter = tickrail_adapters::build(cfg.feed.clone(), 1)?;
        let inst = adapter.instrument().clone();
        let meta = adapter.info();
        eprintln!("[tickrail] {} {}: tick {}, lot {}", meta.venue, inst.symbol, inst.tick_size(), inst.lot_size());
        let stats = Arc::new(FeedStats::default());
        let handle = adapter.start(FeedContext { clock, tx: md_tx, stop: stop.clone(), stats: stats.clone() });
        feed_parts = Some((handle, stats));
        (inst, (meta.adapter, meta.venue, meta.asset_class, meta.endpoint, meta.streams))
    };

    // 2. Strategy and risk.
    let name = cfg.strategy.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let mut params = cfg.strategy.clone();
    params.remove("name");
    let (strategy, param_docs) =
        tickrail_strategies::registry::build(&name, params, &inst).map_err(anyhow::Error::msg)?;
    let risk = RiskEngine::new(risk_limits(&cfg.risk, &inst), kill.clone());
    let fees = FeeSchedule { maker_bps: cfg.execution.maker_fee_bps, taker_bps: cfg.execution.taker_fee_bps };

    // 3. Monitoring and journal plumbing.
    let (stats_tx, stats_rx) = ring::channel::<EngineStats>(64);
    let (lat_tx, lat_rx) = ring::channel::<LatSample>(1 << 16);
    let (journal_tx, journal_handle) = match &cfg.engine.journal {
        Some(path) => {
            let w = JournalWriter::create(path, &inst)?;
            let (tx, rx) = ring::channel::<Record>(1 << 18);
            let stop = stop.clone();
            eprintln!("[tickrail] journal: {}", path.display());
            (Some(tx), Some(spawn_named("journal", move || tickrail_journal::run_writer(rx, w, stop))))
        }
        None => (None, None),
    };

    let (adapter, venue, asset_class, source, streams) = adapter_info;
    let info =
        session_info(&inst, &cfg, sim, &strategy, param_docs, &risk, adapter, venue, asset_class, source, streams);
    let outputs = Outputs { journal: journal_tx, latency: Some(lat_tx), stats: Some(stats_tx) };
    let opts = EngineOptions { replay: false, wait, core: cfg.engine.core };
    let oms = Oms::new(&inst, fees);

    // 4. The engine thread.
    let stop2 = stop.clone();
    let (sim_handle, sim_gateway) = match sim_parts {
        Some((h, g)) => (Some(h), Some(g)),
        None => (None, None),
    };
    let engine_h = match sim_gateway {
        Some(gw) => {
            let engine = Engine::new(inst.clone(), clock, strategy, gw, oms, risk, outputs, opts);
            spawn_named("engine", move || run_loop(engine, md_rx, stop2))
        }
        None => {
            let engine = Engine::new(inst.clone(), clock, strategy, PaperVenue::new(inst.id), oms, risk, outputs, opts);
            spawn_named("engine", move || run_loop(engine, md_rx, stop2))
        }
    };
    eprintln!("[tickrail] {} on {} ({}), orders: {}", info.strategy, inst.symbol, info.venue, info.execution);

    let probe = feed_parts.as_ref().map(|(_, s)| {
        let (a, b) = (s.clone(), s.clone());
        FeedProbe { counters: Arc::new(move || a.counters()), raw: Arc::new(move || b.raw_samples()) }
    });
    let snap = supervise(stats_rx, lat_rx, info, &cfg, stop.clone(), kill, probe)?;

    let final_stats = engine_h.join().expect("engine thread panicked");
    if let Some(h) = sim_handle {
        h.join().expect("simulator thread panicked");
    }
    if let Some((h, s)) = feed_parts {
        h.join().expect("feed thread panicked");
        let c = s.counters();
        let get = |k: &str| c.iter().find(|(n, _)| n == k).map_or(0, |(_, v)| *v);
        eprintln!(
            "[feed] frames={} reconnects={} parse_errors={} dropped={}",
            get("frames"),
            get("reconnects"),
            get("parse_errors"),
            get("dropped")
        );
    }
    let journaled = journal_handle.map(|h| h.join().expect("journal thread panicked"));
    summary::print(&inst, &final_stats, snap.as_ref(), journaled);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn session_info(
    inst: &Instrument,
    cfg: &Config,
    sim: bool,
    strategy: &impl Strategy,
    params: Vec<(String, String, String)>,
    risk: &RiskEngine,
    adapter: String,
    venue: String,
    asset_class: String,
    source: String,
    streams: Vec<(String, String)>,
) -> SessionInfo {
    let dec = inst.qty_decimals as usize;
    let l = &risk.limits;
    let mut params = params;
    params.push((
        "fees".into(),
        format!("maker {} bps, taker {} bps", cfg.execution.maker_fee_bps, cfg.execution.taker_fee_bps),
        "Charged on each fill's notional. Negative maker = rebate.".into(),
    ));
    SessionInfo {
        symbol: inst.symbol.clone(),
        tick_size: inst.tick_size(),
        lot_size: inst.lot_size(),
        price_decimals: inst.price_decimals,
        qty_decimals: inst.qty_decimals,
        mode: if sim { "simulator".into() } else { format!("live data, {} fills", cfg.execution.venue) },
        mode_key: if sim { "sim".into() } else { "live".into() },
        strategy: strategy.name().into(),
        adapter,
        venue,
        asset_class,
        source,
        streams,
        execution: cfg.execution.venue.clone(),
        params,
        limits: vec![
            ("max order size".into(), format!("{:.*}", dec, inst.qty(l.max_order_qty))),
            ("max position".into(), format!("{:.*}", dec, inst.qty(Qty(l.max_position)))),
            ("max open orders".into(), l.max_open_orders.to_string()),
            ("price band".into(), format!("{} ticks from mid", l.price_band_ticks)),
            ("max orders / second".into(), l.max_orders_per_sec.to_string()),
            ("max loss (kill switch)".into(), format!("{}", l.max_loss)),
        ],
        strat_fields: strategy.diagnostic_names().iter().map(|s| s.to_string()).collect(),
        reject_reasons: tickrail_engine::stats::REJECT_REASONS.iter().map(|s| s.to_string()).collect(),
    }
}

/// Runs telemetry on a small tokio runtime until Ctrl-C or the configured duration.
fn supervise(
    stats_rx: Consumer<EngineStats>,
    lat_rx: Consumer<LatSample>,
    info: SessionInfo,
    cfg: &Config,
    stop: Arc<AtomicBool>,
    kill: Arc<AtomicBool>,
    feed: Option<FeedProbe>,
) -> Result<Option<Snapshot>> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("telemetry")
        .enable_all()
        .on_thread_start(|| cpu::configure_current_thread(cpu::ThreadRole::Background, None))
        .build()?;
    let addr = cfg.http.enabled.then(|| cfg.http.listen.clone());
    let ui_dir = cfg.http.ui_dir.clone();
    let duration = cfg.engine.duration_secs;
    rt.block_on(async move {
        let tel = Telemetry { stats_rx, lat_rx, info, kill, feed, ui_dir };
        let handle = tokio::spawn(tel.run(addr, stop.clone()));
        match duration {
            Some(s) => {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(s)) => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            None => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
        eprintln!("\n[tickrail] stopping");
        stop.store(true, Ordering::Relaxed);
        Ok(handle.await.ok().flatten())
    })
}
