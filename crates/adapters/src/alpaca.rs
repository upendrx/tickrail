//! US equities through Alpaca's market-data WebSocket (free account needed).
//!
//! * `quotes` - NBBO (or IEX top of book on the free plan)
//! * `trades` - every trade
//!
//! Credentials come from the config or, more usefully, from the standard
//! `APCA_API_KEY_ID` / `APCA_API_SECRET_KEY` environment variables. The free
//! plan streams the IEX feed (`feed = "iex"`); `sip` needs a paid plan.
//!
//! Equity trades don't say which side was the aggressor, so it is inferred from
//! the latest quote (the Lee-Ready quote rule): at or above the ask is a buy, at
//! or below the bid is a sell, otherwise compare with the mid.

use crate::emit::Emitter;
use crate::ws::{Frame, WsFeed, WsProtocol};
use crate::{AdapterInfo, FeedAdapter, time};
use serde::Deserialize;
use serde_json::value::RawValue;
use tickrail_core::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Ticker, e.g. "AAPL".
    pub symbol: String,
    /// "iex" (free) or "sip" (paid).
    #[serde(default = "default_feed")]
    pub feed: String,
    #[serde(default = "default_ws")]
    pub ws_url: String,
    pub key_id: Option<String>,
    pub secret_key: Option<String>,
    /// US stocks trade in cents above $1.
    #[serde(default = "default_pd")]
    pub price_decimals: u32,
    #[serde(default)]
    pub qty_decimals: u32,
}

fn default_feed() -> String {
    "iex".into()
}
fn default_ws() -> String {
    "wss://stream.data.alpaca.markets/v2".into()
}
fn default_pd() -> u32 {
    2
}

pub fn create(cfg: Config, instrument_id: u32) -> anyhow::Result<Box<dyn FeedAdapter>> {
    let key = cfg.key_id.clone().or_else(|| std::env::var("APCA_API_KEY_ID").ok());
    let secret = cfg.secret_key.clone().or_else(|| std::env::var("APCA_API_SECRET_KEY").ok());
    let (Some(key), Some(secret)) = (key, secret) else {
        anyhow::bail!(
            "alpaca needs credentials: set APCA_API_KEY_ID and APCA_API_SECRET_KEY \
             (free at alpaca.markets) or key_id / secret_key in [feed]"
        );
    };
    let symbol = cfg.symbol.to_uppercase();
    let inst = Instrument {
        id: instrument_id,
        symbol: symbol.clone(),
        price_decimals: cfg.price_decimals,
        qty_decimals: cfg.qty_decimals,
    };
    let url = format!("{}/{}", cfg.ws_url.trim_end_matches('/'), cfg.feed);
    let info = AdapterInfo {
        adapter: "alpaca".into(),
        venue: format!("Alpaca ({})", cfg.feed.to_uppercase()),
        asset_class: "equities".into(),
        endpoint: url.clone(),
        streams: vec![
            (format!("quotes:{symbol}"), "Best bid and offer with sizes".into()),
            (format!("trades:{symbol}"), "Every trade; aggressor inferred from the quote".into()),
        ],
        needs_credentials: true,
    };
    let proto = Proto { url, symbol, key, secret, bid: None, ask: None };
    Ok(Box::new(WsFeed { name: "alpaca", proto, inst, info }))
}

pub struct Proto {
    url: String,
    symbol: String,
    key: String,
    secret: String,
    bid: Option<Price>,
    ask: Option<Price>,
}

#[derive(Deserialize)]
struct Item<'a> {
    #[serde(borrow, rename = "T")]
    kind: &'a str,
    #[serde(borrow, default)]
    msg: Option<&'a str>,
    #[serde(borrow, default)]
    t: Option<&'a str>,
    #[serde(borrow, default)]
    p: Option<&'a RawValue>,
    #[serde(borrow, default)]
    s: Option<&'a RawValue>,
    #[serde(borrow, default)]
    bp: Option<&'a RawValue>,
    #[serde(borrow, default)]
    bs: Option<&'a RawValue>,
    #[serde(borrow, default, rename = "ap")]
    ask_p: Option<&'a RawValue>,
    #[serde(borrow, default, rename = "as")]
    ask_s: Option<&'a RawValue>,
    #[serde(default)]
    code: Option<i64>,
}

/// Equity prices come as JSON numbers; a few sub-dollar names have more
/// decimals than the configured tick, so fall back to rounding for those.
fn price(out: &Emitter, v: Option<&RawValue>) -> Option<Price> {
    let s = v?.get();
    out.px(s).or_else(|| s.parse::<f64>().ok().map(|f| out.instrument().to_ticks(f)))
}

fn size(out: &Emitter, v: Option<&RawValue>) -> Option<Qty> {
    let s = v?.get();
    out.qty(s).or_else(|| s.parse::<f64>().ok().map(|f| out.instrument().to_lots(f)))
}

impl WsProtocol for Proto {
    fn url(&self) -> String {
        self.url.clone()
    }

    fn on_connect(&mut self) -> Vec<String> {
        vec![format!(r#"{{"action":"auth","key":"{}","secret":"{}"}}"#, self.key, self.secret)]
    }

    fn on_text(&mut self, text: &str, out: &mut Emitter) -> Result<Frame, String> {
        let items: Vec<Item> = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut label = "other";
        let mut reply = vec![];
        for it in &items {
            let ts = it.t.and_then(time::rfc3339_ns).unwrap_or_else(Clock::wall_ns);
            match it.kind {
                "success" if it.msg == Some("authenticated") => {
                    let s = &self.symbol;
                    reply.push(format!(r#"{{"action":"subscribe","trades":["{s}"],"quotes":["{s}"]}}"#));
                    label = "auth";
                }
                "success" | "subscription" => label = "control",
                "error" => return Err(format!("Alpaca error {}: {}", it.code.unwrap_or(0), it.msg.unwrap_or(""))),
                "q" => {
                    let (bp, bs) = (price(out, it.bp).ok_or("bad bid")?, size(out, it.bs).ok_or("bad bid size")?);
                    let (ap, asz) =
                        (price(out, it.ask_p).ok_or("bad ask")?, size(out, it.ask_s).ok_or("bad ask size")?);
                    self.bid = Some(bp);
                    self.ask = Some(ap);
                    out.top(Side::Buy, bp, bs, ts);
                    out.top(Side::Sell, ap, asz, ts);
                    label = "quote";
                }
                "t" => {
                    let (p, q) = (price(out, it.p).ok_or("bad price")?, size(out, it.s).ok_or("bad size")?);
                    let side = match (self.bid, self.ask) {
                        (_, Some(a)) if p >= a => Side::Buy,
                        (Some(b), _) if p <= b => Side::Sell,
                        (Some(b), Some(a)) if p.0 * 2 < b.0 + a.0 => Side::Sell,
                        _ => Side::Buy,
                    };
                    out.trade(side, p, q, ts);
                    label = "trade";
                }
                _ => {}
            }
        }
        Ok(Frame { label, reply })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FeedStats;
    use std::sync::Arc;

    #[test]
    fn auth_then_quotes_and_trades() {
        let (tx, mut rx) = ring::channel(128);
        let inst = Instrument { id: 1, symbol: "AAPL".into(), price_decimals: 2, qty_decimals: 0 };
        let mut out = Emitter::new(inst, tx, Arc::new(FeedStats::default()));
        let mut p = Proto {
            url: String::new(),
            symbol: "AAPL".into(),
            key: "k".into(),
            secret: "s".into(),
            bid: None,
            ask: None,
        };
        let f = p.on_text(r#"[{"T":"success","msg":"authenticated"}]"#, &mut out).unwrap();
        assert!(f.reply[0].contains(r#""quotes":["AAPL"]"#));
        let q = r#"[{"T":"q","S":"AAPL","bx":"V","bp":187.52,"bs":2,"ax":"V","ap":187.55,"as":3,"c":["R"],"z":"C","t":"2024-03-01T14:30:00.123456789Z"}]"#;
        p.on_text(q, &mut out).unwrap();
        out.flush();
        assert_eq!(rx.pop().unwrap().kind, MdKind::Top { side: Side::Buy, price: Price(18_752), qty: Qty(2) });
        assert_eq!(rx.pop().unwrap().kind, MdKind::Top { side: Side::Sell, price: Price(18_755), qty: Qty(3) });
        let t =
            r#"[{"T":"t","S":"AAPL","i":1,"x":"V","p":187.55,"s":100,"c":["@"],"z":"C","t":"2024-03-01T14:30:00.2Z"}]"#;
        p.on_text(t, &mut out).unwrap();
        out.flush();
        assert_eq!(rx.pop().unwrap().kind, MdKind::Trade { aggressor: Side::Buy, price: Price(18_755), qty: Qty(100) });
        let sub_penny = r#"[{"T":"t","S":"AAPL","p":187.5213,"s":1,"t":"2024-03-01T14:30:00Z"}]"#;
        p.on_text(sub_penny, &mut out).unwrap();
        out.flush();
        assert_eq!(rx.pop().unwrap().kind, MdKind::Trade { aggressor: Side::Sell, price: Price(18_752), qty: Qty(1) });
    }
}
