# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/). Until 1.0, minor versions may
contain breaking changes.

## [Unreleased]

### Added
- Engine: single-threaded event loop generic over strategy and venue, with
  configurable idle behaviour (`spin`, `yield`, `backoff`, `sleep:<n>us`).
- Feed adapters: Binance, OKX, Kraken (public data), Alpaca (US equities),
  CSV files, and a built-in exchange simulator.
- Strategies: inventory-skewed market maker, order-book-imbalance taker, and a
  registry for adding more.
- Pre-trade risk checks and kill switch; OMS with position, PnL and fees.
- Binary journal with an instrument header; deterministic `replay` from
  journals or CSV files; `export` to CSV.
- Web console and walkthrough guide, served by the binary.
- Python bindings (`pip install ./bindings/python`): order book, journal
  reader, backtests with built-in or Python strategies.
- TOML configuration with `--set key=value` overrides.
