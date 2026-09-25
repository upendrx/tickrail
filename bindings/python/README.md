# tickrail for Python

Python bindings for the [tickrail](https://github.com/upendrx/tickrail) trading
engine: the same Rust order book, journal reader and backtester the engine
uses, with Python strategies running inside the Rust event loop.

```bash
pip install ./bindings/python      # from a tickrail checkout; needs Rust
```

```python
import tickrail

book = tickrail.OrderBook(price_decimals=2, qty_decimals=3)
book.set_level("B", 100.00, 5.0)
book.set_level("S", 100.02, 1.0)
book.microprice()                     # 100.0167

journal = tickrail.read_journal("data/session.journal")   # columnar dicts for Polars/pandas

result = tickrail.backtest("data/session.journal", "market_maker",
                           params={"size": 0.001, "max_inventory": 0.01},
                           max_order_size=0.002, max_position=0.02)
```

See the [Python chapter](https://github.com/upendrx/tickrail/blob/main/docs/src/python.md)
of the developer guide for the full API and writing strategies in Python.
