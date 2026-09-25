# Journals, replay and export

## Journal

With `engine.journal = "path"`, every market-data message, order and execution
report is recorded. The engine pushes records into a ring and a background
thread writes them, so disk speed never affects the engine.

File layout (all little-endian):

```text
magic      8 bytes   "TKRJ0002"
header     u32 price_decimals, u32 qty_decimals, u16 symbol length, symbol bytes
records    48 bytes each
```

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | record type: 1 market data, 2 order, 3 execution |
| 1 | 1 | subtype (level, trade, clear, top / new, cancel / ack, fill, canceled, rejected) |
| 2 | 1 | side |
| 3 | 1 | flags, time in force, liquidity or reject reason |
| 4 | 4 | instrument id |
| 8 | 8 | timestamp (ns): receive time for market data |
| 16 | 8 | sequence number or client order id |
| 24 | 8 | price (ticks) |
| 32 | 8 | quantity (lots) |
| 40 | 8 | venue timestamp (market data) or leaves quantity (fills) |

Fixed-size records mean no parsing and O(1) seek to record *n*. A busy
crypto pair produces about 80 MB per hour.

## Replay

```bash
tickrail replay data/session.journal -c config/binance.toml
tickrail replay data/es.csv -c config/csv-replay.toml
```

Replay feeds recorded market data through the engine with the `[strategy]`,
`[risk]` and `[execution]` sections of the config, using paper fills. The
instrument comes from the journal header. For a CSV, it comes from `[feed]`
`symbol`, `price_decimals` and `qty_decimals`.

Strategies only see event time (`ctx.now`), which in replay is the recorded
receive time (journal) or `ts_ns` (CSV). The same input and settings
therefore always produce the same orders and PnL, and replay can run as fast
as the CPU allows: about 10 to 14 million events per second.

The output also reports decision latency (book update to order sent),
measured on the replay machine.

## Export

```bash
tickrail export data/session.journal session.csv                # every record
tickrail export data/session.journal feed.csv --format feed     # market data, csv-adapter format
```

The events format has columns `record,ts_ns,id,event,side,price,qty,extra`,
with decimal prices and sizes. The feed format is exactly what the `csv`
adapter and `replay` read, so a live session can be turned into a portable
dataset.
