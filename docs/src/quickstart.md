# Quick start

This takes about five minutes and needs no accounts or API keys.

## 1. Run the simulator

```bash
./target/release/tickrail run
```

With no config file, tickrail starts its built-in exchange: a matching engine
with synthetic traders, and the market-making strategy quoting against them.
Open <http://127.0.0.1:8080>. The **Overview** tab summarises in plain words
what the engine is doing, with the live pipeline counters, prices and our
quotes, PnL, the book and the trade tape. The **How it works** page walks
through every calculation with the numbers on screen.

Press Ctrl-C to stop. A session summary is printed on exit.

## 2. Switch to live market data

```bash
./target/release/tickrail run -c config/binance.toml
./target/release/tickrail run -c config/okx.toml --set feed.symbol=ETH-USDT
./target/release/tickrail run -c config/kraken.toml --set feed.symbol=SOL/USD
```

Each of these streams public data and paper-trades against it: orders are
filled by a conservative simulator that only fills a resting order when the
real market trades through its price. Tick and lot sizes are fetched from the
venue at start-up, so any listed symbol works.

`--set section.key=value` overrides any config value. `tickrail list` shows
the adapters and strategies compiled into your build.

## 3. Record and replay

`config/binance.toml` already records a journal. Stop the session after a few
minutes, then replay it with different parameters:

```bash
./target/release/tickrail replay data/binance-btcusdt.journal -c config/binance.toml
./target/release/tickrail replay data/binance-btcusdt.journal -c config/binance.toml \
  --set strategy.half_spread_ticks=60 --set strategy.skew_ticks=100
```

Replay runs the same engine code with paper fills and prints the result in
well under a second for an hour of data. Because strategies only see event
time, the same journal and settings always give the same result.

## 4. Look at the fills in Python

```bash
pip install ./bindings/python polars
python research/analyze.py data/binance-btcusdt.journal
```

This prints fills by side and the average **markout** at 10 ms to 5 s: how
far the price moved after each of our fills. See [market concepts](concepts.md)
for why that's the number to watch.

## 5. Write a strategy in Python

```bash
python examples/python/spread_quoter.py examples/data/sample.csv
```

`examples/python/spread_quoter.py` is a complete market maker in about 40 lines,
running inside the Rust engine through the bindings. Copy it and change
`on_book`. [Strategies](strategies.md) covers the Rust side.

## Next

- [Configuration](configuration.md): every setting.
- [Adapters](adapters.md): equities via Alpaca, your own data via CSV, and how
  to add a venue.
- [Performance](performance.md): getting from microseconds to hundreds of
  nanoseconds on a tuned Linux box.
