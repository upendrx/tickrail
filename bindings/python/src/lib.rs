//! Python module `tickrail._tickrail` (re-exported as `tickrail`).
//!
//! * `OrderBook`: the engine's L2 book, with prices and sizes as floats.
//! * `read_journal(path)`: a journal as columnar dicts, ready for Polars or pandas.
//! * `backtest(data, strategy, ...)`: replays a journal or CSV through the real
//!   engine. `strategy` is a built-in name ("market_maker") or any Python object
//!   with an `on_book(ctx)` method. Python strategies run inside the Rust event
//!   loop, so risk checks, the OMS and the fill model are identical to live runs.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tickrail_book::L2Book;
use tickrail_core::ring;
use tickrail_core::*;
use tickrail_engine::{Engine, EngineOptions, Outputs, PaperVenue};
use tickrail_journal::{JournalReader, Record};
use tickrail_oms::{FeeSchedule, FillInfo, Oms, OrdStatus};
use tickrail_risk::{RiskEngine, RiskLimits};
use tickrail_strategies::{Ctx, Intent, Strategy};

fn side_of(s: &str) -> PyResult<Side> {
    match s {
        "B" | "b" | "buy" | "bid" => Ok(Side::Buy),
        "S" | "s" | "sell" | "ask" => Ok(Side::Sell),
        _ => Err(PyValueError::new_err(format!("side must be 'B' or 'S', got {s:?}"))),
    }
}

fn side_str(s: Side) -> &'static str {
    if s == Side::Buy { "B" } else { "S" }
}

fn inst(symbol: &str, price_decimals: u32, qty_decimals: u32) -> Instrument {
    Instrument { id: 1, symbol: symbol.to_string(), price_decimals, qty_decimals }
}

/// Price-level order book. Prices and sizes are floats in and out, stored as
/// exact integers inside.
#[pyclass(module = "tickrail")]
struct OrderBook {
    book: L2Book,
    inst: Instrument,
}

#[pymethods]
impl OrderBook {
    #[new]
    #[pyo3(signature = (price_decimals = 2, qty_decimals = 8))]
    fn new(price_decimals: u32, qty_decimals: u32) -> Self {
        OrderBook { book: L2Book::default(), inst: inst("BOOK", price_decimals, qty_decimals) }
    }

    /// Sets the total size at a price; size 0 removes the level.
    fn set_level(&mut self, side: &str, price: f64, size: f64) -> PyResult<()> {
        let s = side_of(side)?;
        self.book.set_level(s, self.inst.to_ticks(price), self.inst.to_lots(size));
        Ok(())
    }

    fn clear(&mut self) {
        self.book.clear();
    }

    /// (price, size) of the best bid, or None.
    fn best_bid(&self) -> Option<(f64, f64)> {
        self.book.best_bid().map(|l| (self.inst.px(l.price), self.inst.qty(l.qty)))
    }

    fn best_ask(&self) -> Option<(f64, f64)> {
        self.book.best_ask().map(|l| (self.inst.px(l.price), self.inst.qty(l.qty)))
    }

    fn mid(&self) -> Option<f64> {
        self.book.mid().map(|m| m * self.inst.tick_size())
    }

    /// Size-weighted mid, leaning toward the thinner side.
    fn microprice(&self) -> Option<f64> {
        self.book.microprice().map(|m| m * self.inst.tick_size())
    }

    /// (bid size - ask size) / total over the top `levels`, in [-1, 1].
    #[pyo3(signature = (levels = 5))]
    fn imbalance(&self, levels: usize) -> f64 {
        self.book.imbalance(levels)
    }

    fn spread(&self) -> Option<f64> {
        self.book.spread_ticks().map(|t| t as f64 * self.inst.tick_size())
    }

    /// Top `n` levels of one side, best first, as [(price, size), ...].
    #[pyo3(signature = (side, n = 10))]
    fn levels(&self, side: &str, n: usize) -> PyResult<Vec<(f64, f64)>> {
        let s = side_of(side)?;
        Ok(self.book.levels(s).take(n).map(|l| (self.inst.px(l.price), self.inst.qty(l.qty))).collect())
    }

    fn __repr__(&self) -> String {
        let f = |o: Option<(f64, f64)>| o.map_or("-".to_string(), |(p, q)| format!("{q}@{p}"));
        format!("OrderBook(bid {} | ask {})", f(self.best_bid()), f(self.best_ask()))
    }
}

/// Reads a journal into columnar dicts: `{"symbol", "price_decimals",
/// "qty_decimals", "md": {...}, "orders": {...}, "execs": {...}}`.
/// Each inner dict maps column name to a list, e.g. `polars.DataFrame(j["md"])`.
#[pyfunction]
fn read_journal<'py>(py: Python<'py>, path: &str) -> PyResult<Bound<'py, PyDict>> {
    let reader = JournalReader::open(Path::new(path)).map_err(|e| PyValueError::new_err(format!("{e:#}")))?;
    let inst = reader.instrument.clone();
    let (mut ts, mut ev, mut sd, mut px, mut qt) = (vec![], vec![], vec![], vec![], vec![]);
    let (mut ots, mut oid, mut ocmd, mut oside, mut opx, mut oqty) = (vec![], vec![], vec![], vec![], vec![], vec![]);
    let (mut ets, mut eid, mut ekind, mut epx, mut eqty, mut emaker) = (vec![], vec![], vec![], vec![], vec![], vec![]);
    for r in reader {
        match r {
            Record::Md(m) => {
                let (e, s, p, q) = match m.kind {
                    MdKind::Level { side, price, qty } => ("level", side_str(side), inst.px(price), inst.qty(qty)),
                    MdKind::Top { side, price, qty } => ("top", side_str(side), inst.px(price), inst.qty(qty)),
                    MdKind::Trade { aggressor, price, qty } => {
                        ("trade", side_str(aggressor), inst.px(price), inst.qty(qty))
                    }
                    MdKind::Clear => ("clear", "", f64::NAN, f64::NAN),
                };
                ts.push(m.recv_ts);
                ev.push(e);
                sd.push(s);
                px.push(p);
                qt.push(q);
            }
            Record::Order(o) => {
                ots.push(o.ts);
                match o.cmd {
                    OrderCmd::New { cl_id, side, price, qty, .. } => {
                        oid.push(cl_id);
                        ocmd.push("new");
                        oside.push(side_str(side));
                        opx.push(inst.px(price));
                        oqty.push(inst.qty(qty));
                    }
                    OrderCmd::Cancel { cl_id } => {
                        oid.push(cl_id);
                        ocmd.push("cancel");
                        oside.push("");
                        opx.push(f64::NAN);
                        oqty.push(f64::NAN);
                    }
                }
            }
            Record::Exec(e) => {
                ets.push(e.ts);
                eid.push(e.cl_id);
                let (k, p, q, mk) = match e.kind {
                    ExecKind::Ack => ("ack", f64::NAN, f64::NAN, false),
                    ExecKind::Fill { price, qty, liquidity, .. } => {
                        ("fill", inst.px(price), inst.qty(qty), liquidity == Liquidity::Maker)
                    }
                    ExecKind::Canceled => ("canceled", f64::NAN, f64::NAN, false),
                    ExecKind::Rejected { .. } => ("rejected", f64::NAN, f64::NAN, false),
                };
                ekind.push(k);
                epx.push(p);
                eqty.push(q);
                emaker.push(mk);
            }
        }
    }
    let out = PyDict::new(py);
    out.set_item("symbol", &inst.symbol)?;
    out.set_item("price_decimals", inst.price_decimals)?;
    out.set_item("qty_decimals", inst.qty_decimals)?;
    let md = PyDict::new(py);
    md.set_item("ts_ns", ts)?;
    md.set_item("event", ev)?;
    md.set_item("side", sd)?;
    md.set_item("price", px)?;
    md.set_item("qty", qt)?;
    out.set_item("md", md)?;
    let orders = PyDict::new(py);
    orders.set_item("ts_ns", ots)?;
    orders.set_item("id", oid)?;
    orders.set_item("cmd", ocmd)?;
    orders.set_item("side", oside)?;
    orders.set_item("price", opx)?;
    orders.set_item("qty", oqty)?;
    out.set_item("orders", orders)?;
    let execs = PyDict::new(py);
    execs.set_item("ts_ns", ets)?;
    execs.set_item("id", eid)?;
    execs.set_item("kind", ekind)?;
    execs.set_item("price", epx)?;
    execs.set_item("qty", eqty)?;
    execs.set_item("maker", emaker)?;
    out.set_item("execs", execs)?;
    Ok(out)
}

/// What a Python strategy sees on each call. Prices and sizes are floats.
/// Orders placed through `buy`/`sell`/`cancel` go through the engine's risk
/// checks after the callback returns.
#[pyclass(module = "tickrail")]
struct Context {
    #[pyo3(get)]
    now_ns: u64,
    #[pyo3(get)]
    best_bid: Option<f64>,
    #[pyo3(get)]
    best_ask: Option<f64>,
    #[pyo3(get)]
    bid_size: Option<f64>,
    #[pyo3(get)]
    ask_size: Option<f64>,
    #[pyo3(get)]
    mid: Option<f64>,
    #[pyo3(get)]
    microprice: Option<f64>,
    #[pyo3(get)]
    position: f64,
    #[pyo3(get)]
    pnl: f64,
    #[pyo3(get)]
    tick_size: f64,
    /// [(id, side, price, size, leaves, status)]
    #[pyo3(get)]
    open_orders: Vec<(u64, &'static str, f64, f64, f64, &'static str)>,
    bids: Vec<(f64, f64)>,
    asks: Vec<(f64, f64)>,
    inst: Instrument,
    intents: Vec<Intent>,
}

#[pymethods]
impl Context {
    /// Post a buy order. `post_only` orders are rejected rather than cross the spread.
    #[pyo3(signature = (price, size, post_only = true, ioc = false))]
    fn buy(&mut self, price: f64, size: f64, post_only: bool, ioc: bool) {
        self.order(Side::Buy, price, size, post_only, ioc);
    }

    #[pyo3(signature = (price, size, post_only = true, ioc = false))]
    fn sell(&mut self, price: f64, size: f64, post_only: bool, ioc: bool) {
        self.order(Side::Sell, price, size, post_only, ioc);
    }

    fn cancel(&mut self, order_id: u64) {
        self.intents.push(Intent::Cancel { cl_id: order_id });
    }

    fn cancel_all(&mut self) {
        let ids: Vec<u64> = self.open_orders.iter().filter(|o| o.5 != "pending_cancel").map(|o| o.0).collect();
        self.intents.extend(ids.into_iter().map(|cl_id| Intent::Cancel { cl_id }));
    }

    /// Top `n` levels of one side as [(price, size), ...].
    #[pyo3(signature = (side, n = 10))]
    fn levels(&self, side: &str, n: usize) -> PyResult<Vec<(f64, f64)>> {
        Ok(match side_of(side)? {
            Side::Buy => self.bids.iter().take(n).copied().collect(),
            Side::Sell => self.asks.iter().take(n).copied().collect(),
        })
    }

    /// Rounds a price down to the tick grid.
    fn round_down(&self, price: f64) -> f64 {
        (price / self.tick_size).floor() * self.tick_size
    }

    fn round_up(&self, price: f64) -> f64 {
        (price / self.tick_size).ceil() * self.tick_size
    }
}

impl Context {
    fn order(&mut self, side: Side, price: f64, size: f64, post_only: bool, ioc: bool) {
        let tif = if ioc {
            TimeInForce::Ioc
        } else if post_only {
            TimeInForce::PostOnly
        } else {
            TimeInForce::Gtc
        };
        self.intents.push(Intent::New { side, price: self.inst.to_ticks(price), qty: self.inst.to_lots(size), tif });
    }

    fn build(ctx: &Ctx, inst: &Instrument) -> Self {
        let lvl =
            |side| ctx.book.levels(side).take(20).map(|l| (inst.px(l.price), inst.qty(l.qty))).collect::<Vec<_>>();
        let tick = inst.tick_size();
        let mid = ctx.book.mid();
        Context {
            now_ns: ctx.now,
            best_bid: ctx.book.best_bid().map(|l| inst.px(l.price)),
            best_ask: ctx.book.best_ask().map(|l| inst.px(l.price)),
            bid_size: ctx.book.best_bid().map(|l| inst.qty(l.qty)),
            ask_size: ctx.book.best_ask().map(|l| inst.qty(l.qty)),
            mid: mid.map(|m| m * tick),
            microprice: ctx.book.microprice().map(|m| m * tick),
            position: inst.qty(Qty(ctx.oms.position)),
            pnl: mid.map_or(0.0, |m| ctx.oms.pnl(m)),
            tick_size: tick,
            open_orders: ctx
                .oms
                .orders()
                .map(|o| {
                    let st = match o.status {
                        OrdStatus::PendingNew => "pending_new",
                        OrdStatus::Live => "live",
                        OrdStatus::PendingCancel => "pending_cancel",
                    };
                    (o.cl_id, side_str(o.side), inst.px(o.price), inst.qty(o.qty), inst.qty(o.leaves), st)
                })
                .collect(),
            bids: lvl(Side::Buy),
            asks: lvl(Side::Sell),
            inst: inst.clone(),
            intents: Vec::new(),
        }
    }
}

/// Adapts a Python object to the engine's `Strategy` trait.
struct PyStrategy {
    obj: Py<PyAny>,
    inst: Instrument,
    has_on_fill: bool,
    has_on_trade: bool,
    error: Option<PyErr>,
}

impl PyStrategy {
    fn call(
        &mut self,
        ctx: &mut Ctx,
        method: &str,
        extra: Option<Box<dyn FnOnce(Python<'_>) -> PyResult<Py<PyAny>> + '_>>,
    ) {
        if self.error.is_some() {
            return;
        }
        let inst = self.inst.clone();
        let res = Python::attach(|py| -> PyResult<Vec<Intent>> {
            let c = Py::new(py, Context::build(ctx, &inst))?;
            match extra {
                Some(make) => {
                    let arg = make(py)?;
                    self.obj.call_method1(py, method, (c.clone_ref(py), arg))?;
                }
                None => {
                    self.obj.call_method1(py, method, (c.clone_ref(py),))?;
                }
            }
            Ok(std::mem::take(&mut c.borrow_mut(py).intents))
        });
        match res {
            Ok(intents) => ctx.intents.extend(intents),
            Err(e) => self.error = Some(e),
        }
    }
}

impl Strategy for PyStrategy {
    fn name(&self) -> &'static str {
        "python"
    }

    fn on_book(&mut self, ctx: &mut Ctx) {
        self.call(ctx, "on_book", None);
    }

    fn on_trade(&mut self, ctx: &mut Ctx, price: Price, qty: Qty, aggressor: Side) {
        if self.has_on_trade {
            let (p, q) = (self.inst.px(price), self.inst.qty(qty));
            self.call(
                ctx,
                "on_trade",
                Some(Box::new(move |py| Ok((p, q, side_str(aggressor)).into_pyobject(py)?.into_any().unbind()))),
            );
        }
    }

    fn on_fill(&mut self, ctx: &mut Ctx, fill: &FillInfo) {
        if self.has_on_fill {
            let f = (
                fill.cl_id,
                side_str(fill.side),
                self.inst.px(fill.price),
                self.inst.qty(fill.qty),
                fill.liquidity == Liquidity::Maker,
            );
            self.call(ctx, "on_fill", Some(Box::new(move |py| Ok(f.into_pyobject(py)?.into_any().unbind()))));
        }
    }
}

fn to_toml(v: &Bound<'_, PyAny>) -> PyResult<toml::Value> {
    if let Ok(b) = v.extract::<bool>() {
        Ok(toml::Value::Boolean(b))
    } else if let Ok(i) = v.extract::<i64>() {
        Ok(toml::Value::Integer(i))
    } else if let Ok(f) = v.extract::<f64>() {
        Ok(toml::Value::Float(f))
    } else if let Ok(s) = v.extract::<String>() {
        Ok(toml::Value::String(s))
    } else {
        Err(PyValueError::new_err(format!("unsupported parameter value: {v}")))
    }
}

struct RunResult {
    stats: tickrail_engine::stats::EngineStats,
    events: u64,
    seconds: f64,
    fills: Vec<(u64, u64, Side, f64, f64, bool)>,
    curve: Vec<(u64, f64, f64)>,
}

fn run<S: Strategy>(
    engine: &mut Engine<S, PaperVenue>,
    events: Vec<MdMsg>,
    journal: &mut ring::Consumer<Record>,
    inst: &Instrument,
) -> RunResult {
    let mut fills = Vec::new();
    let mut curve = Vec::new();
    let mut side_of_order = std::collections::HashMap::new();
    let mut drain = |j: &mut ring::Consumer<Record>, fills: &mut Vec<_>| {
        while let Some(r) = j.pop() {
            match r {
                Record::Order(OrderMsg { cmd: OrderCmd::New { cl_id, side, .. }, .. }) => {
                    side_of_order.insert(cl_id, side);
                }
                Record::Exec(ExecMsg { ts, cl_id, kind: ExecKind::Fill { price, qty, liquidity, .. }, .. }) => {
                    let side = side_of_order.get(&cl_id).copied().unwrap_or(Side::Buy);
                    fills.push((ts, cl_id, side, inst.px(price), inst.qty(qty), liquidity == Liquidity::Maker));
                }
                _ => {}
            }
        }
    };
    let started = std::time::Instant::now();
    let mut n = 0u64;
    let mut last_ts = 0;
    for md in events {
        engine.on_md(&md);
        n += 1;
        last_ts = md.recv_ts;
        if n.is_multiple_of(512) {
            drain(journal, &mut fills);
            engine.housekeeping();
            if let Some(m) = engine.book.mid() {
                curve.push((md.recv_ts, engine.oms.pnl(m), inst.qty(Qty(engine.oms.position))));
            }
        }
    }
    let seconds = started.elapsed().as_secs_f64();
    let stats = engine.shutdown();
    drain(journal, &mut fills);
    curve.push((last_ts, stats.pnl, inst.qty(Qty(stats.position_lots))));
    RunResult { stats, events: n, seconds, fills, curve }
}

/// Replays `data` (a `.journal` file or a CSV in the feed format) through the
/// engine with paper fills and returns a dict of results.
///
/// `strategy` is either a built-in name with `params`, e.g.
/// `backtest(path, "market_maker", params={"size": 0.001, "max_inventory": 0.01}, ...)`,
/// or an object with `on_book(ctx)` and optionally `on_trade(ctx, trade)` and
/// `on_fill(ctx, fill)`.
#[pyfunction]
#[pyo3(signature = (
    data, strategy, *, max_order_size, max_position, price_band_ticks = 10_000, max_open_orders = 16,
    max_orders_per_sec = 10_000, max_loss = 1e12, maker_fee_bps = 0.0, taker_fee_bps = 0.0,
    params = None, symbol = None, price_decimals = None, qty_decimals = None
))]
#[allow(clippy::too_many_arguments)]
fn backtest<'py>(
    py: Python<'py>,
    data: &str,
    strategy: &Bound<'py, PyAny>,
    max_order_size: f64,
    max_position: f64,
    price_band_ticks: i64,
    max_open_orders: usize,
    max_orders_per_sec: u32,
    max_loss: f64,
    maker_fee_bps: f64,
    taker_fee_bps: f64,
    params: Option<&Bound<'py, PyDict>>,
    symbol: Option<String>,
    price_decimals: Option<u32>,
    qty_decimals: Option<u32>,
) -> PyResult<Bound<'py, PyDict>> {
    let path = Path::new(data);
    let (instrument, events) = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("csv")) {
        let (Some(pd), Some(qd)) = (price_decimals, qty_decimals) else {
            return Err(PyValueError::new_err("CSV input needs price_decimals= and qty_decimals="));
        };
        let i = inst(symbol.as_deref().unwrap_or("CSV"), pd, qd);
        let ev = tickrail_adapters::csv::read_file(path, &i).map_err(|e| PyValueError::new_err(format!("{e:#}")))?;
        (i, ev)
    } else {
        let r = JournalReader::open(path).map_err(|e| PyValueError::new_err(format!("{e:#}")))?;
        let i = r.instrument.clone();
        (i, r.filter_map(|r| if let Record::Md(m) = r { Some(m) } else { None }).collect())
    };

    let limits = RiskLimits {
        max_order_qty: instrument.to_lots(max_order_size),
        max_position: instrument.to_lots(max_position).0,
        max_open_orders,
        price_band_ticks,
        max_orders_per_sec,
        max_loss,
    };
    let risk = RiskEngine::new(limits, Arc::new(AtomicBool::new(false)));
    let oms = Oms::new(&instrument, FeeSchedule { maker_bps: maker_fee_bps, taker_bps: taker_fee_bps });
    let (jtx, mut jrx) = ring::channel::<Record>(1 << 17);
    let outputs = Outputs { journal: Some(jtx), latency: None, stats: None };
    let opts = EngineOptions { replay: true, ..EngineOptions::default() };
    let venue = PaperVenue::new(instrument.id);

    let (result, name) = if let Ok(name) = strategy.extract::<String>() {
        let mut table = toml::Table::new();
        if let Some(p) = params {
            for (k, v) in p.iter() {
                table.insert(k.extract::<String>()?, to_toml(&v)?);
            }
        }
        let (s, _) = tickrail_strategies::registry::build(&name, table, &instrument).map_err(PyValueError::new_err)?;
        let mut engine = Engine::new(instrument.clone(), Clock::new(), s, venue, oms, risk, outputs, opts);
        // Built-in strategies never touch Python: release the GIL while they run.
        let inst2 = instrument.clone();
        let r = py.detach(move || run(&mut engine, events, &mut jrx, &inst2));
        (r, name)
    } else {
        if !strategy.hasattr("on_book")? {
            return Err(PyValueError::new_err("strategy must be a built-in name or an object with on_book(ctx)"));
        }
        let s = PyStrategy {
            obj: strategy.clone().unbind(),
            inst: instrument.clone(),
            has_on_fill: strategy.hasattr("on_fill")?,
            has_on_trade: strategy.hasattr("on_trade")?,
            error: None,
        };
        let mut engine = Engine::new(instrument.clone(), Clock::new(), s, venue, oms, risk, outputs, opts);
        let r = run(&mut engine, events, &mut jrx, &instrument);
        if let Some(e) = engine.strategy.error.take() {
            return Err(e);
        }
        (r, strategy.get_type().name()?.to_string())
    };

    let s = &result.stats;
    let out = PyDict::new(py);
    out.set_item("symbol", &instrument.symbol)?;
    out.set_item("strategy", name)?;
    out.set_item("events", result.events)?;
    out.set_item("seconds", result.seconds)?;
    out.set_item("orders", s.orders_sent)?;
    out.set_item("cancels", s.cancels_sent)?;
    out.set_item("fills", s.fills)?;
    out.set_item("maker_fills", s.maker_fills)?;
    out.set_item("volume", instrument.qty(Qty(s.volume_lots)))?;
    out.set_item("position", instrument.qty(Qty(s.position_lots)))?;
    out.set_item("pnl", s.pnl)?;
    out.set_item("fees", s.fees)?;
    out.set_item("risk_rejects", s.risk_rejects)?;
    let f = PyDict::new(py);
    f.set_item("ts_ns", result.fills.iter().map(|x| x.0).collect::<Vec<_>>())?;
    f.set_item("id", result.fills.iter().map(|x| x.1).collect::<Vec<_>>())?;
    f.set_item("side", result.fills.iter().map(|x| side_str(x.2)).collect::<Vec<_>>())?;
    f.set_item("price", result.fills.iter().map(|x| x.3).collect::<Vec<_>>())?;
    f.set_item("qty", result.fills.iter().map(|x| x.4).collect::<Vec<_>>())?;
    f.set_item("maker", result.fills.iter().map(|x| x.5).collect::<Vec<_>>())?;
    out.set_item("fill_log", f)?;
    let c = PyDict::new(py);
    c.set_item("ts_ns", result.curve.iter().map(|x| x.0).collect::<Vec<_>>())?;
    c.set_item("pnl", result.curve.iter().map(|x| x.1).collect::<Vec<_>>())?;
    c.set_item("position", result.curve.iter().map(|x| x.2).collect::<Vec<_>>())?;
    out.set_item("equity_curve", c)?;
    Ok(out)
}

#[pymodule]
fn _tickrail(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<OrderBook>()?;
    m.add_class::<Context>()?;
    m.add_function(wrap_pyfunction!(read_journal, m)?)?;
    m.add_function(wrap_pyfunction!(backtest, m)?)?;
    Ok(())
}
