# Configuration

A session is described by one TOML file. Each section picks a component by
name and passes it its settings:

```toml
[feed]        # where market data comes from
[execution]   # where orders go, and fees
[strategy]    # what decides the orders
[risk]        # hard limits, always explicit
[engine]      # threads, idle behaviour, journal
[http]        # web console
[simulator]   # only when feed.adapter = "simulator"
```

Start from one of the files in `config/`. Unknown keys are errors, so a typo
fails at start-up rather than being silently ignored. `tickrail check -c FILE`
validates a file without running it.

## Overrides

Any value can be set on the command line, and `--set` can be repeated:

```bash
tickrail run -c config/binance.toml --set feed.symbol=ETHUSDT --set engine.wait=spin
```

The value is parsed as TOML (so `5`, `2.5`, `true` and `[1, 2]` keep their
types) and falls back to a string.

## `[feed]`

| Key | Meaning |
|---|---|
| `adapter` | `simulator`, `binance`, `okx`, `kraken`, `alpaca` or `csv` |
| *adapter keys* | Everything else is passed to the adapter; see [adapters](adapters.md) |

## `[execution]`

| Key | Default | Meaning |
|---|---|---|
| `venue` | `"paper"` | `paper` (simulated fills on live data) or `simulator` (only with the simulator feed) |
| `maker_fee_bps` | `0.0` | Fee on maker fills, in basis points of notional. Negative is a rebate. |
| `taker_fee_bps` | `0.0` | Fee on taker fills |

## `[strategy]`

| Key | Meaning |
|---|---|
| `name` | `market_maker` or `imbalance` (see `tickrail list`) |
| *strategy keys* | Passed to the strategy; see [strategies](strategies.md) |

Sizes are in instrument units (0.001 BTC, 10 shares), and prices and
distances in ticks.

## `[risk]`

Risk limits have no defaults for sizes: you always state them.

| Key | Default | Meaning |
|---|---|---|
| `max_order_size` | required | Largest single order, instrument units |
| `max_position` | required | Largest worst-case position (position plus every open order on one side) |
| `price_band_ticks` | required | Reject orders further than this from the mid |
| `max_loss` | required | Kill switch trips when PnL falls below minus this (quote currency) |
| `max_open_orders` | `16` | Open orders at once |
| `max_orders_per_sec` | `200` | New orders per one-second window (event time) |

## `[engine]`

| Key | Default | Meaning |
|---|---|---|
| `wait` | `"backoff"` | Idle behaviour of polling threads: `spin`, `yield`, `backoff`, `sleep:<n>us` or `sleep:<n>ms`. See [performance](performance.md). |
| `core` | none | Pin the engine thread to this CPU core (Linux) |
| `journal` | none | Record every event to this file |
| `duration_secs` | none | Stop after this long (otherwise run until Ctrl-C) |
| `md_ring` | `65536` | Capacity of the market-data ring, in messages |

## `[http]`

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | Serve the console and API |
| `listen` | `"127.0.0.1:8080"` | Address to bind. The console has no authentication; see [deployment](deployment.md). |
| `ui_dir` | none | Serve the UI from this folder instead of the built-in copy, so you can edit it without recompiling |

## `[simulator]`

Used when `feed.adapter = "simulator"`, which also requires
`execution.venue = "simulator"`.

| Key | Default | Meaning |
|---|---|---|
| `symbol` | `"SIM"` | Display name |
| `price_decimals`, `qty_decimals` | `2`, `3` | Tick is 10^-price_decimals, lot 10^-qty_decimals |
| `start_price` | `60000.0` | Initial fair value |
| `event_rate` | `20000` | Background order events per second |
| `latency_us` | `20` | One-way latency of our orders to the matching engine |
| `volatility` | `150` | Hidden fair-value volatility, ticks per √second |
| `seed` | `42` | Same seed and settings give the same market |
| `core` | none | Pin the simulator thread (Linux) |
