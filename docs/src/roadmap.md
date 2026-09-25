# Roadmap

Roughly in priority order. Discussion and pull requests welcome on any of
them.

## Execution

- **Order gateways** behind the `Venue` trait: exchange testnets first
  (Binance, OKX), then authenticated live trading with position
  reconciliation on start-up and on a timer.
- **Queue-position model** for the paper venue, so passive fills at the
  touch are estimated rather than ignored.
- Order latency model for paper fills on live data.

## Market data

- Full-depth books: Binance diff-depth with REST snapshot resync, Kraken
  checksum verification.
- More venues: Bybit, Coinbase Advanced Trade, Deribit, Hyperliquid;
  Interactive Brokers and Databento for traditional markets.
- Binary exchange protocols (ITCH, SBE/MDP3) with kernel-bypass receive
  (AF_XDP, ef_vi) behind the same `FeedAdapter` trait.

## Engine

- Multiple instruments per process, with one engine thread per shard and
  cross-instrument signals.
- Slab-backed OMS (no hashing on the hot path).
- TSC clock on x86, PTP-disciplined timestamps.
- Aeron for inter-process messaging and a replicated risk service.

## Tooling

- Parameter-sweep command for `replay` (grid search, multiple journals in parallel).
- Parquet export.
- Prometheus metrics endpoint.
