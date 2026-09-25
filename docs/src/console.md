# Web console

With `[http] enabled = true` (the default), the binary serves a console at
`http://127.0.0.1:8080`. It's static HTML and JavaScript, updated over a
WebSocket ten times a second.

| Tab | Shows |
|---|---|
| Overview | A plain-language summary of what the engine is doing, the pipeline counters, price with our quotes, PnL and position, the book and the trade tape |
| Order book & trades | Depth chart, the full ladder, tape, and buy/sell flow balance |
| Strategy | Where the strategy wants to quote and why, internal values, parameters |
| Orders & fills | Working orders, recent fills with markouts, order/cancel/fill ratios |
| Risk | Limit usage meters, rejects by reason, the kill switch |
| Latency | Percentiles per path and the tick-to-order histogram |
| Market explorer | Every Binance spot pair live, straight from Binance's public API to the browser |
| Data sources | The adapter's streams, counters and the latest raw frames |
| Guide & glossary | Commands and terms |

`/guide` is a step-by-step walkthrough of the pipeline that fills in every
formula with the live numbers.

## API

| Endpoint | |
|---|---|
| `GET /api/snapshot` | The latest snapshot as JSON (session info, stats, book, orders, fills, latency, feed counters) |
| `GET /ws` | WebSocket pushing the same snapshot on every update |
| `POST /api/kill` | Trip the kill switch |
| `POST /api/unkill` | Clear it |

The snapshot is a stable place to hook in your own monitoring (Grafana, a
Slack bot, a notebook).

## Customising

The UI lives in `ui/` and is compiled into the binary. Set
`http.ui_dir = "ui"` to serve it from disk instead, then edit and reload the
browser without rebuilding.

## Security

There is no authentication. Keep `listen` on `127.0.0.1`, and use an SSH tunnel
or an authenticating reverse proxy for remote access. Anyone who can reach the
port can trip or clear the kill switch.
