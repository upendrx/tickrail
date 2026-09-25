"""Python interface to the tickrail engine.

    >>> import tickrail
    >>> book = tickrail.OrderBook(price_decimals=2, qty_decimals=3)
    >>> book.set_level("B", 100.00, 5.0)
    >>> book.set_level("S", 100.02, 1.0)
    >>> round(book.microprice(), 4)
    100.0167

The order book, journal reader and backtester are the same Rust code the
engine runs; Python strategies are called from inside the Rust event loop.
"""

from ._tickrail import OrderBook, Context, backtest, read_journal, __version__

__all__ = ["OrderBook", "Context", "backtest", "read_journal", "__version__"]
