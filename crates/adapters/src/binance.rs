//! Binance spot, public streams (no API key).
//!
//! * `<sym>@depth20@100ms` - top 20 levels per side, full snapshot every 100 ms
//! * `<sym>@bookTicker`    - best bid/ask on every change
//! * `<sym>@trade`         - every trade
//!
//! Works with any Binance-compatible host, e.g. `wss://stream.binance.us:9443`
//! plus `https://api.binance.us` for Binance.US.

use crate::emit::Emitter;
use crate::ws::{Frame, WsFeed, WsProtocol};
use crate::{AdapterInfo, FeedAdapter, http};
use anyhow::Context;
use serde::Deserialize;
use serde_json::value::RawValue;
use tickrail_core::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub symbol: String,
    #[serde(default = "default_ws")]
    pub ws_url: String,
    #[serde(default = "default_rest")]
    pub rest_url: String,
    /// Skip the exchangeInfo lookup by giving precision explicitly.
    pub price_decimals: Option<u32>,
    pub qty_decimals: Option<u32>,
}

fn default_ws() -> String {
    "wss://data-stream.binance.vision".into()
}
fn default_rest() -> String {
    "https://data-api.binance.vision".into()
}

pub fn create(cfg: Config, instrument_id: u32) -> anyhow::Result<Box<dyn FeedAdapter>> {
    let symbol = cfg.symbol.to_uppercase();
    let (pd, qd) = match (cfg.price_decimals, cfg.qty_decimals) {
        (Some(p), Some(q)) => (p, q),
        (p, q) => {
            let (ap, aq) = lookup(&cfg.rest_url, &symbol)?;
            (p.unwrap_or(ap), q.unwrap_or(aq))
        }
    };
    let inst = Instrument { id: instrument_id, symbol: symbol.clone(), price_decimals: pd, qty_decimals: qd };
    let s = symbol.to_lowercase();
    let streams = [format!("{s}@depth20@100ms"), format!("{s}@bookTicker"), format!("{s}@trade")];
    let info = AdapterInfo {
        adapter: "binance".into(),
        venue: "Binance".into(),
        asset_class: "crypto".into(),
        endpoint: cfg.ws_url.clone(),
        streams: vec![
            (streams[0].clone(), "Top 20 levels per side, full snapshot every 100 ms".into()),
            (streams[1].clone(), "Best bid and ask, pushed on every change".into()),
            (streams[2].clone(), "Every trade, with the aggressor side".into()),
        ],
        needs_credentials: false,
    };
    let url = format!("{}/stream?streams={}", cfg.ws_url.trim_end_matches('/'), streams.join("/"));
    Ok(Box::new(WsFeed { name: "binance", proto: Proto { url }, inst, info }))
}

fn lookup(rest: &str, symbol: &str) -> anyhow::Result<(u32, u32)> {
    let v = http::get_json(&format!("{}/api/v3/exchangeInfo?symbol={symbol}", rest.trim_end_matches('/')))?;
    let filters = v["symbols"][0]["filters"].as_array().with_context(|| format!("Binance has no symbol {symbol}"))?;
    let step = |ty: &str, key: &str| {
        filters.iter().find(|f| f["filterType"] == ty).and_then(|f| f[key].as_str()).map(http::decimals_of)
    };
    Ok((
        step("PRICE_FILTER", "tickSize").context("no PRICE_FILTER")?,
        step("LOT_SIZE", "stepSize").context("no LOT_SIZE")?,
    ))
}

pub struct Proto {
    url: String,
}

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(borrow)]
    stream: &'a str,
    #[serde(borrow)]
    data: &'a RawValue,
}

#[derive(Deserialize)]
struct Depth<'a> {
    #[serde(borrow)]
    bids: Vec<[&'a str; 2]>,
    #[serde(borrow)]
    asks: Vec<[&'a str; 2]>,
}

#[derive(Deserialize)]
struct BookTicker<'a> {
    #[serde(borrow, rename = "b")]
    bid: &'a str,
    #[serde(borrow, rename = "B")]
    bid_qty: &'a str,
    #[serde(borrow, rename = "a")]
    ask: &'a str,
    #[serde(borrow, rename = "A")]
    ask_qty: &'a str,
}

#[derive(Deserialize)]
struct Trade<'a> {
    #[serde(rename = "T")]
    time_ms: u64,
    #[serde(borrow, rename = "p")]
    price: &'a str,
    #[serde(borrow, rename = "q")]
    qty: &'a str,
    /// Buyer was the maker, so the seller was the aggressor.
    #[serde(rename = "m")]
    buyer_is_maker: bool,
}

const BAD: &str = "bad number";

impl WsProtocol for Proto {
    fn url(&self) -> String {
        self.url.clone()
    }

    fn on_connect(&mut self) -> Vec<String> {
        vec![] // streams are in the URL
    }

    fn on_text(&mut self, text: &str, out: &mut Emitter) -> Result<Frame, String> {
        let env: Envelope = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let data = env.data.get();
        if env.stream.ends_with("@trade") {
            let t: Trade = serde_json::from_str(data).map_err(|e| e.to_string())?;
            let side = if t.buyer_is_maker { Side::Sell } else { Side::Buy };
            let (p, q) = (out.px(t.price).ok_or(BAD)?, out.qty(t.qty).ok_or(BAD)?);
            out.trade(side, p, q, t.time_ms * 1_000_000);
            Ok(Frame::data("trade"))
        } else if env.stream.ends_with("@bookTicker") {
            let t: BookTicker = serde_json::from_str(data).map_err(|e| e.to_string())?;
            let ts = Clock::wall_ns();
            let (bp, bq) = (out.px(t.bid).ok_or(BAD)?, out.qty(t.bid_qty).ok_or(BAD)?);
            let (ap, aq) = (out.px(t.ask).ok_or(BAD)?, out.qty(t.ask_qty).ok_or(BAD)?);
            out.top(Side::Buy, bp, bq, ts);
            out.top(Side::Sell, ap, aq, ts);
            Ok(Frame::data("bookTicker"))
        } else if env.stream.contains("@depth") {
            let d: Depth = serde_json::from_str(data).map_err(|e| e.to_string())?;
            let ts = Clock::wall_ns();
            out.clear(ts);
            for (side, rows) in [(Side::Buy, &d.bids), (Side::Sell, &d.asks)] {
                for [p, q] in rows {
                    let (p, q) = (out.px(p).ok_or(BAD)?, out.qty(q).ok_or(BAD)?);
                    out.level(side, p, q, ts);
                }
            }
            Ok(Frame::data("depth"))
        } else {
            Err(format!("unexpected stream {}", env.stream))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FeedStats;
    use std::sync::Arc;

    #[test]
    fn parses_all_streams() {
        let (tx, mut rx) = ring::channel(128);
        let inst = Instrument { id: 1, symbol: "BTCUSDT".into(), price_decimals: 2, qty_decimals: 5 };
        let mut out = Emitter::new(inst, tx, Arc::new(FeedStats::default()));
        let mut p = Proto { url: String::new() };

        let depth = r#"{"stream":"btcusdt@depth20@100ms","data":{"lastUpdateId":1,"bids":[["60000.10000000","0.50000000"]],"asks":[["60000.20000000","1.25000000"]]}}"#;
        p.on_text(depth, &mut out).unwrap();
        out.flush();
        assert_eq!(rx.pop().unwrap().kind, MdKind::Clear);
        assert_eq!(
            rx.pop().unwrap().kind,
            MdKind::Level { side: Side::Buy, price: Price(6_000_010), qty: Qty(50_000) }
        );
        let last = rx.pop().unwrap();
        assert!(last.is_last_in_batch());

        let bt = r#"{"stream":"btcusdt@bookTicker","data":{"u":1,"s":"BTCUSDT","b":"60000.10000000","B":"2.00000000","a":"60000.30000000","A":"0.10000000"}}"#;
        assert_eq!(p.on_text(bt, &mut out).unwrap().label, "bookTicker");
        out.flush();
        assert_eq!(rx.pop().unwrap().kind, MdKind::Top { side: Side::Buy, price: Price(6_000_010), qty: Qty(200_000) });
        assert!(rx.pop().unwrap().is_last_in_batch());

        let tr = r#"{"stream":"btcusdt@trade","data":{"e":"trade","E":1,"s":"BTCUSDT","t":5,"p":"60000.15000000","q":"0.00100000","T":1700000000000,"m":true,"M":true}}"#;
        p.on_text(tr, &mut out).unwrap();
        out.flush();
        assert_eq!(
            rx.pop().unwrap().kind,
            MdKind::Trade { aggressor: Side::Sell, price: Price(6_000_015), qty: Qty(100) }
        );
    }
}
