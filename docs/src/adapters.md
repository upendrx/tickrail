# Market-data adapters

An adapter turns one venue's feed into the engine's normalised `MdMsg` stream:
book levels, top-of-book updates, trades and snapshot boundaries, all in
integer ticks and lots. It runs on its own thread and talks to the engine only
through a lock-free ring.

| Adapter | Market | Access | Data used |
|---|---|---|---|
| `simulator` | Built-in exchange | offline | L2 updates and trades from a real matching engine |
| `binance` | Binance spot (and Binance.US) | public | `depth20@100ms`, `bookTicker`, `trade` |
| `okx` | OKX spot | public | `books5`, `bbo-tbt`, `trades` |
| `kraken` | Kraken spot | public | `book` (10 to 1000 levels, incremental), `trade` |
| `alpaca` | US equities | free account | `quotes`, `trades` (IEX free, SIP paid) |
| `csv` | Anything | local file | levels, top of book, trades |

Every network adapter reconnects with backoff, counts parse errors and dropped
messages, and exposes raw frames to the console's Data sources tab.

## binance

```toml
[feed]
adapter = "binance"
symbol = "BTCUSDT"
# ws_url = "wss://data-stream.binance.vision"      # default, market data only
# rest_url = "https://data-api.binance.vision"
# price_decimals = 2                               # skip the exchangeInfo lookup
# qty_decimals = 5
```

For Binance.US set `ws_url = "wss://stream.binance.us:9443"` and
`rest_url = "https://api.binance.us"`.

## okx

```toml
[feed]
adapter = "okx"
symbol = "BTC-USDT"
```

OKX drops idle connections after 30 s, so the adapter sends an application
`ping` every 20 s.

## kraken

```toml
[feed]
adapter = "kraken"
symbol = "BTC/USD"
depth = 25            # 10, 25, 100, 500 or 1000
```

Kraken sends prices as JSON numbers. The adapter parses the raw number text
exactly, like the string prices other venues use. The book checksum isn't
verified yet.

## alpaca

```toml
[feed]
adapter = "alpaca"
symbol = "AAPL"
feed = "iex"          # "sip" with a paid plan
```

Set `APCA_API_KEY_ID` and `APCA_API_SECRET_KEY` in the environment (or
`key_id` and `secret_key` in the file, which is not recommended). Equity
trades don't say which side was the aggressor, so the adapter infers it from
the latest quote (at or above the ask is a buy, at or below the bid a sell).
Data only flows while the US market is open.

## csv

```toml
[feed]
adapter = "csv"
path = "data/es-2024-05-01.csv"
symbol = "ES"
price_decimals = 2
qty_decimals = 0
speed = 1.0           # 0 = as fast as possible
```

The file format:

```text
ts_ns,event,side,price,qty
1714000000000000000,clear,,,
1714000000000000000,level,B,101.25,300
1714000000000000000,level,S,101.26,120
1714000000150000000,trade,S,101.25,50
1714000000200000000,top,B,101.24,800
```

- `event`: `level` (absolute size at a price, 0 removes it), `top` (new best
  level, clears better stale levels), `trade`, or `clear` (start of a snapshot).
- `side`: `B` or `S` (`buy`/`sell` also work). For trades it is the aggressor.
- Rows sharing a `ts_ns` are applied as one batch before the strategy runs.

`tickrail export JOURNAL out.csv --format feed` writes this format from any
journal, and `tickrail replay file.csv` backtests it directly at full speed.

## Writing an adapter

Most venues are JSON over WebSocket. For those you implement
`ws::WsProtocol`, and the shared runner handles TLS, reconnects,
keep-alives, batching and statistics:

```rust
pub trait WsProtocol: Send + 'static {
    fn url(&self) -> String;
    /// Frames to send after connecting: subscriptions, auth.
    fn on_connect(&mut self) -> Vec<String>;
    /// Parse one frame into `out`. Return a label for the message type,
    /// plus any frames to send back.
    fn on_text(&mut self, text: &str, out: &mut Emitter) -> Result<Frame, String>;
    /// Application-level ping, if the venue needs one.
    fn keepalive(&self) -> Option<(Duration, String)> { None }
}
```

`Emitter` collects the messages produced from one frame and pushes them to
the engine as one consistent batch when the frame is done:

```rust
fn on_text(&mut self, text: &str, out: &mut Emitter) -> Result<Frame, String> {
    let msg: MyTrade = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let price = out.px(msg.price).ok_or("bad price")?;   // exact decimal to ticks
    let qty = out.qty(msg.size).ok_or("bad size")?;
    out.trade(if msg.taker_sells { Side::Sell } else { Side::Buy }, price, qty, msg.ts_ns);
    Ok(Frame::data("trade"))
}
```

Steps:

1. Create `crates/adapters/src/<venue>.rs` with a `Config` struct
   (`#[serde(deny_unknown_fields)]`) and a `create(cfg, instrument_id)`
   function. Look up tick and lot size from the venue's REST API with
   `http::get_json`, and allow overriding them from config.
2. Implement `WsProtocol` and return a `WsFeed { name, proto, inst, info }`.
   `AdapterInfo` is what the console shows: venue name, endpoint, streams.
3. Add a cargo feature and a match arm in `lib.rs`.
4. Write unit tests that feed real frames from the venue's documentation into
   `on_text` and check the emitted messages. See the tests at the bottom of
   `okx.rs`.
5. Add `config/<venue>.toml`.

For something that isn't a WebSocket (a binary multicast feed, a vendor SDK,
a file format), implement `FeedAdapter` directly: `start` gets a
`FeedContext` with the output ring, the stop flag and the stats block, and
spawns its own thread. `csv.rs` is the example.

Rules for adapters: parse on the adapter thread, never block the engine (drop
and count when the ring is full), set `LAST_IN_BATCH` only when the book is
consistent (the `Emitter` does this per frame), and keep venue timestamps in
`exch_ts` so latency to the venue can be measured.
