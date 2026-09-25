# Introduction

tickrail is a trading engine built the way electronic market makers build
theirs: one busy thread that owns all trading state, lock-free queues between
threads, integer prices, and a journal that makes every session replayable.
It is written in Rust, with Python bindings for research and a web console for
watching it work.

It is meant for developers and quants who want to build, test and understand
low-latency trading systems on real market data: crypto exchanges, US
equities, or any market you can export to CSV.

## What you get

- **An engine** that turns market data into orders in a few hundred
  nanoseconds of software time: order book, strategy, pre-trade risk and order
  management on one thread, with no allocation on the hot path.
- **Pluggable market data.** Binance, OKX and Kraken with public data and no
  keys, US equities via Alpaca, CSV files for anything else, and a built-in
  exchange simulator. Each adapter is one file behind a small trait.
- **Pluggable strategies.** An inventory-aware market maker and an
  order-book-imbalance taker ship built in. Write your own in Rust, or in
  Python and run it inside the same event loop.
- **Paper execution** against live data, or a full price-time-priority
  matching engine with synthetic traders, so strategies meet queue position,
  partial fills and adverse selection before they meet real money.
- **Deterministic replay** of recorded journals or CSV files at millions of
  events per second, for backtests and regression tests.
- **A web console** with the book, quotes, fills, markouts, risk usage and
  latency histograms, plus a guide that walks through every calculation with
  live numbers.
- **One config file** that picks every component by name, runs on anything
  from a Raspberry Pi to a tuned colocated server, with idle behaviour you
  choose per machine.

## What it isn't (yet)

tickrail does not send orders to exchanges. Execution is paper (simulated
fills against real data) or the built-in simulator. Real order gateways are
the next big piece of work; see the [roadmap](roadmap.md). Until then, nothing
here can lose real money, and nothing here should be read as a claim that a
strategy is profitable.

## How to read this book

If you're new to market microstructure, read [market concepts](concepts.md)
after the [quick start](quickstart.md). It explains the terms the rest of the
book uses. If you want to extend the engine, [architecture](architecture.md),
[adapters](adapters.md) and [strategies](strategies.md) are the chapters that
matter.
