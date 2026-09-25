//! Monitoring, entirely off the hot path.
//!
//! The engine does two cheap things: it pushes raw latency samples, and every
//! 100 ms a `Copy` stats snapshot, into SPSC rings (dropping when full). This
//! crate turns those into HDR histograms and JSON, and serves the web UI.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use hdrhistogram::Histogram;
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tickrail_core::ring::Consumer;
use tokio::sync::watch;

pub use tickrail_engine::stats::*;

#[derive(Clone, Debug, Default, Serialize)]
pub struct LatSummary {
    pub count: u64,
    pub p50: u64,
    pub p90: u64,
    pub p99: u64,
    pub p999: u64,
    pub max: u64,
}

fn summarize(h: &Histogram<u64>) -> LatSummary {
    if h.is_empty() {
        return LatSummary::default();
    }
    LatSummary {
        count: h.len(),
        p50: h.value_at_quantile(0.50),
        p90: h.value_at_quantile(0.90),
        p99: h.value_at_quantile(0.99),
        p999: h.value_at_quantile(0.999),
        max: h.max(),
    }
}

/// Static description of the running session, shown on the dashboard.
#[derive(Clone, Debug, Serialize, Default)]
pub struct SessionInfo {
    pub symbol: String,
    pub tick_size: f64,
    pub lot_size: f64,
    pub price_decimals: u32,
    pub qty_decimals: u32,
    pub mode: String,
    /// `sim`, `live` or `replay`.
    pub mode_key: String,
    pub strategy: String,
    /// Feed adapter name ("binance", "okx", "csv", "simulator", ...).
    pub adapter: String,
    pub venue: String,
    pub asset_class: String,
    /// Where market data comes from (URL or file).
    pub source: String,
    /// (stream or channel, what it carries)
    pub streams: Vec<(String, String)>,
    /// Where orders go ("paper", "simulator", ...).
    pub execution: String,
    /// (name, value, what it does)
    pub params: Vec<(String, String, String)>,
    /// (name, value)
    pub limits: Vec<(String, String)>,
    pub strat_fields: Vec<String>,
    pub reject_reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub info: SessionInfo,
    pub stats: EngineStats,
    pub md_rate: f64,
    pub order_rate: f64,
    pub tick_to_order: LatSummary,
    pub md_queue: LatSummary,
    pub order_ack: LatSummary,
    pub md_process: LatSummary,
    /// Latency histogram buckets for tick→order: (upper bound ns, count).
    pub t2o_buckets: Vec<(u64, u64)>,
    pub feed: Vec<(String, u64)>,
    /// Latest raw frame per message type, as received from the venue.
    pub raw: Vec<(String, String)>,
    pub uptime_s: f64,
}

/// Read-only view of the feed thread's counters, provided by the host.
#[derive(Clone)]
pub struct FeedProbe {
    pub counters: Arc<dyn Fn() -> Vec<(String, u64)> + Send + Sync>,
    pub raw: Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>,
}

/// Log-spaced buckets (≈3 per decade) from 40 ns to 100 ms.
fn buckets(h: &Histogram<u64>) -> Vec<(u64, u64)> {
    let mut edges = Vec::new();
    let mut e = 40.0f64;
    while e < 1e8 {
        edges.push(e as u64);
        e *= 2.154;
    }
    let mut out = Vec::with_capacity(edges.len());
    let mut lo = 0u64;
    for &hi in &edges {
        out.push((hi, h.count_between(lo, hi.saturating_sub(1))));
        lo = hi;
    }
    out
}

/// Aggregator + web server. Call from inside a tokio runtime.
pub struct Telemetry {
    pub stats_rx: Consumer<EngineStats>,
    pub lat_rx: Consumer<LatSample>,
    pub info: SessionInfo,
    pub kill: Arc<AtomicBool>,
    pub feed: Option<FeedProbe>,
    /// Serve the UI from this folder instead of the copy built into the binary,
    /// so the dashboard can be edited without recompiling.
    pub ui_dir: Option<std::path::PathBuf>,
}

#[derive(Clone)]
struct AppState {
    snap: watch::Receiver<Option<Snapshot>>,
    kill: Arc<AtomicBool>,
    ui_dir: Option<std::path::PathBuf>,
}

const EMBEDDED_INDEX: &str = include_str!("../../../ui/index.html");
const EMBEDDED_GUIDE: &str = include_str!("../../../ui/guide.html");

/// Reads a UI page from `ui_dir` if configured, otherwise the embedded copy.
fn page(ui_dir: &Option<std::path::PathBuf>, file: &str, embedded: &'static str) -> Html<String> {
    if let Some(dir) = ui_dir
        && let Ok(s) = std::fs::read_to_string(dir.join(file))
    {
        return Html(s);
    }
    Html(embedded.to_string())
}

impl Telemetry {
    /// Runs the aggregator loop and HTTP server until `stop`. Returns the final snapshot.
    pub async fn run(self, addr: Option<String>, stop: Arc<AtomicBool>) -> Option<Snapshot> {
        let (snap_tx, snap_rx) = watch::channel(None::<Snapshot>);
        if let Some(addr) = addr {
            let state = AppState { snap: snap_rx.clone(), kill: self.kill.clone(), ui_dir: self.ui_dir.clone() };
            let app = Router::new()
                .route(
                    "/",
                    get(|State(st): State<AppState>| async move { page(&st.ui_dir, "index.html", EMBEDDED_INDEX) }),
                )
                .route(
                    "/guide",
                    get(|State(st): State<AppState>| async move { page(&st.ui_dir, "guide.html", EMBEDDED_GUIDE) }),
                )
                .route("/api/snapshot", get(api_snapshot))
                .route("/api/kill", post(api_kill))
                .route("/api/unkill", post(api_unkill))
                .route("/ws", get(ws_handler))
                .with_state(state);
            match tokio::net::TcpListener::bind(&addr).await {
                Ok(listener) => {
                    eprintln!("[telemetry] dashboard on http://{addr}");
                    tokio::spawn(async move {
                        let _ = axum::serve(listener, app).await;
                    });
                }
                Err(e) => eprintln!("[telemetry] cannot bind {addr}: {e}"),
            }
        }

        let Telemetry { mut stats_rx, mut lat_rx, info, feed, .. } = self;
        let mk = || Histogram::<u64>::new_with_bounds(1, 60_000_000_000, 3).expect("histogram");
        let (mut t2o, mut mdq, mut ack, mut mdp) = (mk(), mk(), mk(), mk());
        let mut order_base: Option<(u64, u64)> = None;
        let mut order_rate = 0.0;
        let started = std::time::Instant::now();
        let mut last: Option<EngineStats> = None;
        let mut rate_base: Option<(u64, u64)> = None;
        let mut md_rate = 0.0;
        let mut last_reset = std::time::Instant::now();
        let mut out = None;
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tick.tick().await;
            while let Some(s) = lat_rx.pop() {
                let h = match s.kind {
                    LatKind::TickToOrder => &mut t2o,
                    LatKind::MdQueue => &mut mdq,
                    LatKind::OrderAck => &mut ack,
                    LatKind::MdProcess => &mut mdp,
                };
                let _ = h.record(s.ns.max(1));
            }
            while let Some(s) = stats_rx.pop() {
                last = Some(s);
            }
            if let Some(s) = last {
                match rate_base {
                    Some((ts, n)) if s.ts_ns > ts + 900_000_000 => {
                        md_rate = (s.md_msgs - n) as f64 / ((s.ts_ns - ts) as f64 * 1e-9);
                        rate_base = Some((s.ts_ns, s.md_msgs));
                    }
                    None => rate_base = Some((s.ts_ns, s.md_msgs)),
                    _ => {}
                }
                match order_base {
                    Some((ts, n)) if s.ts_ns > ts + 900_000_000 => {
                        order_rate = (s.orders_sent - n) as f64 / ((s.ts_ns - ts) as f64 * 1e-9);
                        order_base = Some((s.ts_ns, s.orders_sent));
                    }
                    None => order_base = Some((s.ts_ns, s.orders_sent)),
                    _ => {}
                }
                let snap = Snapshot {
                    info: info.clone(),
                    stats: s,
                    md_rate,
                    order_rate,
                    tick_to_order: summarize(&t2o),
                    md_queue: summarize(&mdq),
                    order_ack: summarize(&ack),
                    md_process: summarize(&mdp),
                    t2o_buckets: buckets(&t2o),
                    feed: feed.as_ref().map(|f| (f.counters)()).unwrap_or_default(),
                    raw: feed.as_ref().map(|f| (f.raw)()).unwrap_or_default(),
                    uptime_s: started.elapsed().as_secs_f64(),
                };
                out = Some(snap.clone());
                let _ = snap_tx.send(Some(snap));
            }
            // Rolling 30 s window so the dashboard reflects current behaviour.
            if last_reset.elapsed() > Duration::from_secs(30) && !stop.load(Ordering::Relaxed) {
                t2o.reset();
                mdq.reset();
                ack.reset();
                mdp.reset();
                last_reset = std::time::Instant::now();
            }
            if stop.load(Ordering::Relaxed) {
                // One more pass after the engine's final snapshot.
                tokio::time::sleep(Duration::from_millis(150)).await;
                while let Some(s) = stats_rx.pop() {
                    last = Some(s);
                }
                if let (Some(s), Some(o)) = (last, out.as_mut()) {
                    o.stats = s;
                }
                return out;
            }
        }
    }
}

async fn api_snapshot(State(st): State<AppState>) -> impl IntoResponse {
    Json(st.snap.borrow().clone())
}

async fn api_kill(State(st): State<AppState>) -> &'static str {
    st.kill.store(true, Ordering::Relaxed);
    "killed"
}

async fn api_unkill(State(st): State<AppState>) -> &'static str {
    st.kill.store(false, Ordering::Relaxed);
    "resumed"
}

async fn ws_handler(ws: WebSocketUpgrade, State(st): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_loop(socket, st.snap))
}

async fn ws_loop(mut socket: WebSocket, mut snap: watch::Receiver<Option<Snapshot>>) {
    while snap.changed().await.is_ok() {
        let json = match &*snap.borrow_and_update() {
            Some(s) => serde_json::to_string(s).unwrap_or_default(),
            None => continue,
        };
        if socket.send(Message::Text(json.into())).await.is_err() {
            break;
        }
    }
}
