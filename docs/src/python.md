# Python

The `tickrail` Python package wraps the same Rust crates the engine uses.

```bash
pip install ./bindings/python        # needs a Rust toolchain; builds with maturin
```

The release workflow also builds wheels for Linux, macOS and Windows.

## OrderBook

```python
import tickrail

book = tickrail.OrderBook(price_decimals=2, qty_decimals=3)
book.set_level("B", 100.00, 5.0)      # size 0 removes the level
book.set_level("S", 100.02, 1.0)
book.best_bid()        # (100.0, 5.0)
book.microprice()      # 100.0167
book.imbalance(5)      # 0.667
book.levels("B", 10)   # [(100.0, 5.0)]
```

## read_journal

```python
import polars as pl

j = tickrail.read_journal("data/session.journal")
md = pl.DataFrame(j["md"])            # ts_ns, event, side, price, qty
orders = pl.DataFrame(j["orders"])    # ts_ns, id, cmd, side, price, qty
execs = pl.DataFrame(j["execs"])      # ts_ns, id, kind, price, qty, maker
```

Columns are plain lists, so pandas works the same way (`pd.DataFrame(j["md"])`).

## backtest

```python
result = tickrail.backtest(
    "data/session.journal",           # or a CSV in the csv-adapter format
    "market_maker",                   # a built-in, or any object with on_book(ctx)
    params={"size": 0.001, "max_inventory": 0.01, "half_spread_ticks": 20},
    max_order_size=0.002,
    max_position=0.02,
    price_band_ticks=50_000,          # optional; see signature below
    maker_fee_bps=0.0,
    taker_fee_bps=2.0,
)
result["pnl"], result["fills"], result["orders"]
pl.DataFrame(result["fill_log"])      # every fill
pl.DataFrame(result["equity_curve"])  # pnl and position over time
```

Keyword arguments: `max_order_size`, `max_position` (required),
`price_band_ticks`, `max_open_orders`, `max_orders_per_sec`, `max_loss`,
`maker_fee_bps`, `taker_fee_bps`, `params`, and for CSV input `symbol`,
`price_decimals`, `qty_decimals`.

Built-in strategies run with the GIL released.

## Python strategies

Pass any object with an `on_book(ctx)` method. Optional callbacks are
`on_trade(ctx, (price, size, aggressor))` and
`on_fill(ctx, (order_id, side, price, size, maker))`.

`ctx` (a `tickrail.Context`) has:

| | |
|---|---|
| `now_ns` | event time |
| `best_bid`, `best_ask`, `bid_size`, `ask_size`, `mid`, `microprice` | floats or `None` |
| `position`, `pnl`, `tick_size` | |
| `open_orders` | `[(id, side, price, size, leaves, status)]` |
| `levels(side, n)` | `[(price, size)]`, best first |
| `buy(price, size, post_only=True, ioc=False)`, `sell(...)` | queue an order |
| `cancel(id)`, `cancel_all()` | queue cancels |
| `round_down(price)`, `round_up(price)` | snap to the tick grid |

Orders go through the engine's risk checks after the callback returns, exactly
as in a live session. An exception inside the strategy stops the backtest and
is re-raised from `backtest()`.

See `examples/python/spread_quoter.py` for a complete example.
