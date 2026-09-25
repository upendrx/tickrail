"""A market maker written in Python, running inside the Rust engine.

    pip install ./bindings/python
    python examples/python/spread_quoter.py examples/data/sample.csv

The strategy joins the best bid and ask and leans its quotes against its
inventory. The engine handles risk checks, order state and fills; the
Python code only decides prices.
"""

import sys

import tickrail


class SpreadQuoter:
    def __init__(self, size, max_inventory, edge_ticks=0, skew_ticks=2):
        self.size = size
        self.max_inventory = max_inventory
        self.edge = edge_ticks
        self.skew = skew_ticks

    def on_book(self, ctx):
        if ctx.best_bid is None or ctx.best_ask is None:
            return
        tick = ctx.tick_size
        lean = -ctx.position / self.max_inventory * self.skew * tick
        want = {
            "B": ctx.round_down(ctx.best_bid - self.edge * tick + lean),
            "S": ctx.round_up(ctx.best_ask + self.edge * tick + lean),
        }
        if ctx.position + self.size > self.max_inventory:
            want.pop("B")
        if ctx.position - self.size < -self.max_inventory:
            want.pop("S")

        working = {}
        for order_id, side, price, _size, _leaves, status in ctx.open_orders:
            if status == "pending_cancel":
                working.setdefault(side, None)
                continue
            if side in want and abs(price - want[side]) < tick / 2 and side not in working:
                working[side] = order_id
            else:
                ctx.cancel(order_id)
        for side, price in want.items():
            if side not in working:
                (ctx.buy if side == "B" else ctx.sell)(price, self.size)

    def on_fill(self, ctx, fill):
        order_id, side, price, size, maker = fill
        print(f"fill #{order_id}: {side} {size} @ {price} ({'maker' if maker else 'taker'})")


def main(path):
    kwargs = dict(max_order_size=20, max_position=200, price_decimals=2, qty_decimals=0, symbol="SAMPLE")
    res = tickrail.backtest(path, SpreadQuoter(size=10, max_inventory=100), **kwargs)
    print(f"\n{res['strategy']}: {res['events']} events in {res['seconds'] * 1e3:.1f} ms")
    print(f"orders {res['orders']}  cancels {res['cancels']}  fills {res['fills']}  position {res['position']}  pnl {res['pnl']:.2f}")

    builtin = tickrail.backtest(path, "market_maker", params={"size": 10, "max_inventory": 100, "half_spread_ticks": 1}, **kwargs)
    print(f"built-in market_maker for comparison: fills {builtin['fills']}  pnl {builtin['pnl']:.2f}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "examples/data/sample.csv")
