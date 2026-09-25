//! Kraken spot, WebSocket API v2, public channels (no API key).
//!
//! * `book` (depth 25) - snapshot, then incremental level updates
//! * `trade`           - every trade, with the taker side
//!
//! Kraken sends prices as JSON numbers, not strings. We read the raw number
//! text and parse it exactly, the same as string prices. Symbols look like `BTC/USD`.

use crate::emit::Emitter;
use crate::ws::{Frame, WsFeed, WsProtocol};
use crate::{AdapterInfo, FeedAdapter, http, time};
use anyhow::Context;
use serde::Deserialize;
use serde_json::value::RawValue;
use tickrail_core::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// e.g. "BTC/USD", "ETH/EUR"
    pub symbol: String,
    #[serde(default = "default_ws")]
    pub ws_url: String,
    #[serde(default = "default_rest")]
    pub rest_url: String,
    /// Book depth: 10, 25, 100, 500 or 1000.
    #[serde(default = "default_depth")]
    pub depth: u32,
    pub price_decimals: Option<u32>,
    pub qty_decimals: Option<u32>,
}

fn default_ws() -> String {
    "wss://ws.kraken.com/v2".into()
}
fn default_rest() -> String {
    "https://api.kraken.com".into()
}
fn default_depth() -> u32 {
    25
}

pub fn create(cfg: Config, instrument_id: u32) -> anyhow::Result<Box<dyn FeedAdapter>> {
    let symbol = cfg.symbol.to_uppercase();
    let (pd, qd) = match (cfg.price_decimals, cfg.qty_decimals) {
        (Some(p), Some(q)) => (p, q),
        (p, q) => {
            let v =
                http::get_json(&format!("{}/0/public/AssetPairs?pair={symbol}", cfg.rest_url.trim_end_matches('/')))?;
            let pair = v["result"]
                .as_object()
                .and_then(|m| m.values().next())
                .with_context(|| format!("Kraken has no pair {symbol}"))?;
            let pdec = pair["pair_decimals"].as_u64().context("no pair_decimals")? as u32;
            let ldec = pair["lot_decimals"].as_u64().context("no lot_decimals")? as u32;
            (p.unwrap_or(pdec), q.unwrap_or(ldec))
        }
    };
    let inst = Instrument { id: instrument_id, symbol: symbol.clone(), price_decimals: pd, qty_decimals: qd };
    let info = AdapterInfo {
        adapter: "kraken".into(),
        venue: "Kraken".into(),
        asset_class: "crypto".into(),
        endpoint: cfg.ws_url.clone(),
        streams: vec![
            (format!("book:{symbol} depth {}", cfg.depth), "Snapshot, then every level change".into()),
            (format!("trade:{symbol}"), "Every trade, with the taker side".into()),
        ],
        needs_credentials: false,
    };
    Ok(Box::new(WsFeed { name: "kraken", proto: Proto { url: cfg.ws_url, symbol, depth: cfg.depth }, inst, info }))
}

pub struct Proto {
    url: String,
    symbol: String,
    depth: u32,
}

#[derive(Deserialize)]
struct Msg<'a> {
    #[serde(borrow, default)]
    channel: Option<&'a str>,
    #[serde(borrow, default, rename = "type")]
    kind: Option<&'a str>,
    #[serde(borrow, default)]
    data: Vec<Data<'a>>,
    #[serde(default)]
    success: Option<bool>,
    #[serde(borrow, default)]
    error: Option<&'a str>,
}

#[derive(Deserialize)]
struct Data<'a> {
    #[serde(borrow, default)]
    bids: Vec<Level<'a>>,
    #[serde(borrow, default)]
    asks: Vec<Level<'a>>,
    #[serde(borrow, default)]
    price: Option<&'a RawValue>,
    #[serde(borrow, default)]
    qty: Option<&'a RawValue>,
    #[serde(borrow, default)]
    side: Option<&'a str>,
    #[serde(borrow, default)]
    timestamp: Option<&'a str>,
}

#[derive(Deserialize)]
struct Level<'a> {
    #[serde(borrow)]
    price: &'a RawValue,
    #[serde(borrow)]
    qty: &'a RawValue,
}

const BAD: &str = "bad number";

impl WsProtocol for Proto {
    fn url(&self) -> String {
        self.url.clone()
    }

    fn on_connect(&mut self) -> Vec<String> {
        let s = &self.symbol;
        vec![
            format!(
                r#"{{"method":"subscribe","params":{{"channel":"book","symbol":["{s}"],"depth":{}}}}}"#,
                self.depth
            ),
            format!(r#"{{"method":"subscribe","params":{{"channel":"trade","symbol":["{s}"],"snapshot":false}}}}"#),
        ]
    }

    fn on_text(&mut self, text: &str, out: &mut Emitter) -> Result<Frame, String> {
        let m: Msg = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if m.success == Some(false) {
            return Err(format!("Kraken rejected a request: {}", m.error.unwrap_or(text)));
        }
        let now = Clock::wall_ns();
        match m.channel {
            Some("book") => {
                let snapshot = m.kind == Some("snapshot");
                for d in &m.data {
                    let ts = d.timestamp.and_then(time::rfc3339_ns).unwrap_or(now);
                    if snapshot {
                        out.clear(ts);
                    }
                    for (side, rows) in [(Side::Buy, &d.bids), (Side::Sell, &d.asks)] {
                        for l in rows {
                            let (p, q) = (out.px(l.price.get()).ok_or(BAD)?, out.qty(l.qty.get()).ok_or(BAD)?);
                            out.level(side, p, q, ts);
                        }
                    }
                }
                Ok(Frame::data(if snapshot { "book snapshot" } else { "book update" }))
            }
            Some("trade") => {
                for d in &m.data {
                    let ts = d.timestamp.and_then(time::rfc3339_ns).unwrap_or(now);
                    let side = if d.side == Some("sell") { Side::Sell } else { Side::Buy };
                    let p = out.px(d.price.ok_or(BAD)?.get()).ok_or(BAD)?;
                    let q = out.qty(d.qty.ok_or(BAD)?.get()).ok_or(BAD)?;
                    out.trade(side, p, q, ts);
                }
                Ok(Frame::data("trade"))
            }
            Some("heartbeat") => Ok(Frame::data("heartbeat")),
            Some(other) => Ok(Frame::data(if other == "status" { "status" } else { "other" })),
            None => Ok(Frame::data("ack")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FeedStats;
    use std::sync::Arc;

    #[test]
    fn parses_book_and_trades() {
        let (tx, mut rx) = ring::channel(128);
        let inst = Instrument { id: 1, symbol: "BTC/USD".into(), price_decimals: 1, qty_decimals: 8 };
        let mut out = Emitter::new(inst, tx, Arc::new(FeedStats::default()));
        let mut p = Proto { url: String::new(), symbol: "BTC/USD".into(), depth: 25 };
        let snap = r#"{"channel":"book","type":"snapshot","data":[{"symbol":"BTC/USD","bids":[{"price":45283.5,"qty":0.10000000}],"asks":[{"price":45285.2,"qty":0.00100000}],"checksum":3310070434}]}"#;
        p.on_text(snap, &mut out).unwrap();
        out.flush();
        assert_eq!(rx.pop().unwrap().kind, MdKind::Clear);
        assert_eq!(
            rx.pop().unwrap().kind,
            MdKind::Level { side: Side::Buy, price: Price(452_835), qty: Qty(10_000_000) }
        );
        assert!(rx.pop().unwrap().is_last_in_batch());
        let upd = r#"{"channel":"book","type":"update","data":[{"symbol":"BTC/USD","bids":[{"price":45283.5,"qty":0}],"asks":[],"checksum":1,"timestamp":"2023-10-06T17:35:55.440295Z"}]}"#;
        p.on_text(upd, &mut out).unwrap();
        out.flush();
        assert_eq!(rx.pop().unwrap().kind, MdKind::Level { side: Side::Buy, price: Price(452_835), qty: Qty(0) });
        let tr = r#"{"channel":"trade","type":"update","data":[{"symbol":"BTC/USD","side":"sell","price":45283.4,"qty":0.0005,"ord_type":"market","trade_id":1,"timestamp":"2023-10-06T17:35:55.440295Z"}]}"#;
        p.on_text(tr, &mut out).unwrap();
        out.flush();
        assert_eq!(
            rx.pop().unwrap().kind,
            MdKind::Trade { aggressor: Side::Sell, price: Price(452_834), qty: Qty(50_000) }
        );
        assert_eq!(p.on_text(r#"{"channel":"heartbeat"}"#, &mut out).unwrap().label, "heartbeat");
    }
}
