//! OKX spot, public channels (no API key).
//!
//! * `books5`  - top 5 levels per side, full snapshot on change (up to every 100 ms)
//! * `bbo-tbt` - best bid/ask, tick by tick
//! * `trades`  - every trade
//!
//! OKX closes idle connections after 30 s, so the runner sends a text `ping`
//! every 20 s. Instrument ids look like `BTC-USDT`.

use crate::emit::Emitter;
use crate::ws::{Frame, WsFeed, WsProtocol};
use crate::{AdapterInfo, FeedAdapter, http};
use anyhow::Context;
use serde::Deserialize;
use std::time::Duration;
use tickrail_core::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// e.g. "BTC-USDT"
    pub symbol: String,
    #[serde(default = "default_ws")]
    pub ws_url: String,
    #[serde(default = "default_rest")]
    pub rest_url: String,
    pub price_decimals: Option<u32>,
    pub qty_decimals: Option<u32>,
}

fn default_ws() -> String {
    "wss://ws.okx.com:8443/ws/v5/public".into()
}
fn default_rest() -> String {
    "https://www.okx.com".into()
}

pub fn create(cfg: Config, instrument_id: u32) -> anyhow::Result<Box<dyn FeedAdapter>> {
    let symbol = cfg.symbol.to_uppercase();
    let (pd, qd) = match (cfg.price_decimals, cfg.qty_decimals) {
        (Some(p), Some(q)) => (p, q),
        (p, q) => {
            let url = format!(
                "{}/api/v5/public/instruments?instType=SPOT&instId={symbol}",
                cfg.rest_url.trim_end_matches('/')
            );
            let v = http::get_json(&url)?;
            let d = &v["data"][0];
            let tick = d["tickSz"].as_str().with_context(|| format!("OKX has no spot instrument {symbol}"))?;
            let lot = d["lotSz"].as_str().context("no lotSz")?;
            (p.unwrap_or(http::decimals_of(tick)), q.unwrap_or(http::decimals_of(lot)))
        }
    };
    let inst = Instrument { id: instrument_id, symbol: symbol.clone(), price_decimals: pd, qty_decimals: qd };
    let info = AdapterInfo {
        adapter: "okx".into(),
        venue: "OKX".into(),
        asset_class: "crypto".into(),
        endpoint: cfg.ws_url.clone(),
        streams: vec![
            (format!("books5:{symbol}"), "Top 5 levels per side, full snapshot on change".into()),
            (format!("bbo-tbt:{symbol}"), "Best bid and ask, tick by tick".into()),
            (format!("trades:{symbol}"), "Every trade, with the taker side".into()),
        ],
        needs_credentials: false,
    };
    Ok(Box::new(WsFeed { name: "okx", proto: Proto { url: cfg.ws_url, symbol }, inst, info }))
}

pub struct Proto {
    url: String,
    symbol: String,
}

#[derive(Deserialize)]
struct Push<'a> {
    #[serde(borrow)]
    arg: Arg<'a>,
    #[serde(borrow)]
    data: Vec<Data<'a>>,
}

#[derive(Deserialize)]
struct Arg<'a> {
    #[serde(borrow)]
    channel: &'a str,
}

#[derive(Deserialize)]
struct Data<'a> {
    #[serde(borrow, default)]
    bids: Vec<Vec<&'a str>>,
    #[serde(borrow, default)]
    asks: Vec<Vec<&'a str>>,
    #[serde(borrow, default)]
    px: Option<&'a str>,
    #[serde(borrow, default)]
    sz: Option<&'a str>,
    #[serde(borrow, default)]
    side: Option<&'a str>,
    #[serde(borrow)]
    ts: &'a str,
}

const BAD: &str = "bad number";

impl WsProtocol for Proto {
    fn url(&self) -> String {
        self.url.clone()
    }

    fn on_connect(&mut self) -> Vec<String> {
        let s = &self.symbol;
        vec![format!(
            r#"{{"op":"subscribe","args":[{{"channel":"books5","instId":"{s}"}},{{"channel":"bbo-tbt","instId":"{s}"}},{{"channel":"trades","instId":"{s}"}}]}}"#
        )]
    }

    fn keepalive(&self) -> Option<(Duration, String)> {
        Some((Duration::from_secs(20), "ping".into()))
    }

    fn on_text(&mut self, text: &str, out: &mut Emitter) -> Result<Frame, String> {
        if text == "pong" {
            return Ok(Frame::data("pong"));
        }
        if text.contains(r#""event""#) {
            if text.contains(r#""error""#) {
                return Err(format!("OKX error: {text}"));
            }
            return Ok(Frame::data("event"));
        }
        let push: Push = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let label = match push.arg.channel {
            "trades" => "trades",
            "books5" => "books5",
            "bbo-tbt" => "bbo-tbt",
            other => return Err(format!("unexpected channel {other}")),
        };
        for d in &push.data {
            let ts = d.ts.parse::<u64>().map_err(|_| BAD)? * 1_000_000;
            match label {
                "trades" => {
                    let side = if d.side == Some("sell") { Side::Sell } else { Side::Buy };
                    let (p, q) = (out.px(d.px.ok_or(BAD)?).ok_or(BAD)?, out.qty(d.sz.ok_or(BAD)?).ok_or(BAD)?);
                    out.trade(side, p, q, ts);
                }
                "books5" => {
                    out.clear(ts);
                    for (side, rows) in [(Side::Buy, &d.bids), (Side::Sell, &d.asks)] {
                        for r in rows {
                            let (p, q) =
                                (out.px(r.first().ok_or(BAD)?).ok_or(BAD)?, out.qty(r.get(1).ok_or(BAD)?).ok_or(BAD)?);
                            out.level(side, p, q, ts);
                        }
                    }
                }
                _ => {
                    for (side, rows) in [(Side::Buy, &d.bids), (Side::Sell, &d.asks)] {
                        if let Some(r) = rows.first() {
                            let (p, q) =
                                (out.px(r.first().ok_or(BAD)?).ok_or(BAD)?, out.qty(r.get(1).ok_or(BAD)?).ok_or(BAD)?);
                            out.top(side, p, q, ts);
                        }
                    }
                }
            }
        }
        Ok(Frame::data(label))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FeedStats;
    use std::sync::Arc;

    #[test]
    fn parses_channels() {
        let (tx, mut rx) = ring::channel(128);
        let inst = Instrument { id: 1, symbol: "BTC-USDT".into(), price_decimals: 1, qty_decimals: 8 };
        let mut out = Emitter::new(inst, tx, Arc::new(FeedStats::default()));
        let mut p = Proto { url: String::new(), symbol: "BTC-USDT".into() };
        let t = r#"{"arg":{"channel":"trades","instId":"BTC-USDT"},"data":[{"instId":"BTC-USDT","tradeId":"1","px":"42219.9","sz":"0.12060306","side":"sell","ts":"1630048897897"}]}"#;
        p.on_text(t, &mut out).unwrap();
        out.flush();
        assert_eq!(
            rx.pop().unwrap().kind,
            MdKind::Trade { aggressor: Side::Sell, price: Price(422_199), qty: Qty(12_060_306) }
        );
        let b = r#"{"arg":{"channel":"books5","instId":"BTC-USDT"},"data":[{"asks":[["8476.9","415","0","13"]],"bids":[["8476.8","2","0","1"]],"instId":"BTC-USDT","ts":"1597026383085"}]}"#;
        p.on_text(b, &mut out).unwrap();
        out.flush();
        assert_eq!(rx.pop().unwrap().kind, MdKind::Clear);
        assert_eq!(
            rx.pop().unwrap().kind,
            MdKind::Level { side: Side::Buy, price: Price(84_768), qty: Qty(200_000_000) }
        );
        assert!(rx.pop().unwrap().is_last_in_batch());
        assert_eq!(p.on_text(r#"{"event":"subscribe","arg":{"channel":"trades"}}"#, &mut out).unwrap().label, "event");
    }
}
