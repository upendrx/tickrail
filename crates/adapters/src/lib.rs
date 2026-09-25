//! Market-data adapters.
//!
//! An adapter turns one venue's feed into the normalised [`MdMsg`] stream the
//! engine consumes. Adapters are picked by name in the `[feed]` section of the
//! config file, and each one reads its own settings from that same table.
//!
//! Most venues speak JSON over WebSocket, so they only implement the small
//! [`ws::WsProtocol`] trait (URL, subscribe messages, parse one frame) and reuse
//! the shared runner for TLS, reconnects, keep-alives and batching.
//!
//! Adding a venue: create `src/<venue>.rs`, implement [`FeedAdapter`] (usually
//! via `ws::WsFeed`), and add an arm to [`build`]. See `docs/src/adapters.md`.

pub mod emit;
pub mod http;
pub mod time;
pub mod ws;

#[cfg(feature = "alpaca")]
pub mod alpaca;
#[cfg(feature = "binance")]
pub mod binance;
#[cfg(feature = "csv")]
pub mod csv;
#[cfg(feature = "kraken")]
pub mod kraken;
#[cfg(feature = "okx")]
pub mod okx;

use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tickrail_core::ring::Producer;
use tickrail_core::{Clock, Instrument, MdMsg};

/// Human-readable description of a running feed, shown in the dashboard.
#[derive(Clone, Debug, Serialize)]
pub struct AdapterInfo {
    pub adapter: String,
    pub venue: String,
    /// "crypto", "equities", "file", ...
    pub asset_class: String,
    pub endpoint: String,
    /// (stream or channel, what it carries)
    pub streams: Vec<(String, String)>,
    pub needs_credentials: bool,
}

/// Everything a feed thread needs from the host process.
pub struct FeedContext {
    pub clock: Clock,
    pub tx: Producer<MdMsg>,
    pub stop: Arc<AtomicBool>,
    pub stats: Arc<FeedStats>,
}

pub trait FeedAdapter: Send {
    fn info(&self) -> AdapterInfo;
    fn instrument(&self) -> &Instrument;
    /// Starts the feed on its own thread. Must return when `ctx.stop` is set.
    fn start(self: Box<Self>, ctx: FeedContext) -> JoinHandle<()>;
}

/// Counters shared between the feed thread and the monitoring side.
#[derive(Default)]
pub struct FeedStats {
    pub connected: AtomicBool,
    pub frames: AtomicU64,
    pub messages: AtomicU64,
    pub reconnects: AtomicU64,
    pub parse_errors: AtomicU64,
    pub dropped: AtomicU64,
    /// Local receive time minus venue timestamp of the latest trade, in ns.
    /// Includes network delay and any offset between the two clocks.
    pub venue_to_local_ns: AtomicU64,
    /// Latest raw frame per message type, sampled a few times per second.
    raw: Mutex<HashMap<&'static str, (u64, String)>>,
    per_type: Mutex<HashMap<&'static str, u64>>,
}

impl FeedStats {
    /// Records a raw frame for display. Cheap enough for the feed thread: it
    /// only copies when the previous sample of this type is older than 250 ms.
    pub fn sample_raw(&self, label: &'static str, now_ns: u64, text: &str) {
        if let Ok(mut m) = self.per_type.lock() {
            *m.entry(label).or_default() += 1;
        }
        let Ok(mut raw) = self.raw.lock() else { return };
        let stale = raw.get(label).is_none_or(|(t, _)| now_ns.saturating_sub(*t) > 250_000_000);
        if stale {
            let cut = text.char_indices().nth(1_500).map_or(text.len(), |(i, _)| i);
            raw.insert(label, (now_ns, text[..cut].to_string()));
        }
    }

    pub fn raw_samples(&self) -> Vec<(String, String)> {
        let raw = self.raw.lock().map(|m| m.clone()).unwrap_or_default();
        let mut v: Vec<_> = raw.into_iter().map(|(k, (_, t))| (k.to_string(), t)).collect();
        v.sort();
        v
    }

    pub fn counters(&self) -> Vec<(String, u64)> {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let mut v = vec![
            ("connected".to_string(), self.connected.load(Ordering::Relaxed) as u64),
            ("frames".into(), g(&self.frames)),
            ("messages".into(), g(&self.messages)),
            ("reconnects".into(), g(&self.reconnects)),
            ("parse_errors".into(), g(&self.parse_errors)),
            ("dropped".into(), g(&self.dropped)),
            ("venue_to_local_ns".into(), g(&self.venue_to_local_ns)),
        ];
        if let Ok(m) = self.per_type.lock() {
            let mut types: Vec<_> = m.iter().map(|(k, n)| (format!("frames:{k}"), *n)).collect();
            types.sort();
            v.extend(types);
        }
        v
    }
}

/// Names of the adapters compiled into this build.
#[allow(clippy::vec_init_then_push)]
pub fn available() -> Vec<&'static str> {
    let mut v = vec![];
    #[cfg(feature = "binance")]
    v.push("binance");
    #[cfg(feature = "okx")]
    v.push("okx");
    #[cfg(feature = "kraken")]
    v.push("kraken");
    #[cfg(feature = "alpaca")]
    v.push("alpaca");
    #[cfg(feature = "csv")]
    v.push("csv");
    v
}

/// Builds the adapter named in `table["adapter"]`, passing it the rest of the table.
pub fn build(mut table: toml::Table, instrument_id: u32) -> anyhow::Result<Box<dyn FeedAdapter>> {
    let name = match table.remove("adapter") {
        Some(toml::Value::String(s)) => s,
        _ => anyhow::bail!("[feed] needs `adapter = \"...\"` (one of: {})", available().join(", ")),
    };
    let v = toml::Value::Table(table);
    let parse_err = |e: toml::de::Error| anyhow::anyhow!("[feed] {name}: {e}");
    let _ = (&v, instrument_id, &parse_err);
    match name.as_str() {
        #[cfg(feature = "binance")]
        "binance" => binance::create(v.try_into().map_err(parse_err)?, instrument_id),
        #[cfg(feature = "okx")]
        "okx" => okx::create(v.try_into().map_err(parse_err)?, instrument_id),
        #[cfg(feature = "kraken")]
        "kraken" => kraken::create(v.try_into().map_err(parse_err)?, instrument_id),
        #[cfg(feature = "alpaca")]
        "alpaca" => alpaca::create(v.try_into().map_err(parse_err)?, instrument_id),
        #[cfg(feature = "csv")]
        "csv" => csv::create(v.try_into().map_err(parse_err)?, instrument_id),
        other => anyhow::bail!("unknown feed adapter `{other}` (available: {})", available().join(", ")),
    }
}
