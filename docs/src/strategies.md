# Strategies

A strategy is a plain struct the engine calls synchronously on its thread.
It sees a read-only view of the book and the OMS and returns **intents**
(new order, cancel). The engine assigns order ids, runs risk checks, updates
the OMS and talks to the venue.

## Built-in strategies

### `market_maker`

Two-sided quoting with inventory skew, after Avellaneda and Stoikov (2008):

```text
fair value   = microprice
reservation  = fair value − (position / max_inventory) × skew_ticks
half spread  = half_spread_ticks + vol_mult × σ          σ: EWMA of mid changes
bid, ask     = floor(reservation − half), ceil(reservation + half)   never crossing the touch
```

Quotes are post-only, and replaced only when the target moves at least
`requote_ticks` and no more often than `min_requote_us` per side. That keeps
queue priority and stays well inside rate limits.

| Key | Default | Meaning |
|---|---|---|
| `size` | required | Quote size, instrument units |
| `max_inventory` | required | Stop quoting the side that would exceed this position |
| `half_spread_ticks` | `2.0` | Minimum distance from reservation to each quote |
| `vol_mult` | `1.5` | Extra half-spread per tick of recent volatility |
| `skew_ticks` | `10.0` | Reservation shift at full inventory |
| `requote_ticks` | `2` | Minimum target move before replacing a quote |
| `min_requote_us` | `10000` | Minimum time between new quotes on one side |

### `imbalance`

Crosses the spread with an IOC when the top of book is one-sided, and
flattens when the signal fades.

| Key | Default | Meaning |
|---|---|---|
| `size` | required | Order size |
| `max_inventory` | required | Largest position |
| `threshold` | `0.6` | Imbalance (−1 to 1) needed to trade |
| `levels` | `3` | Levels per side the imbalance is measured over |
| `cooldown_ms` | `50` | Minimum time between orders |

## Writing a strategy in Rust

```rust
use tickrail_core::*;
use tickrail_strategies::{Ctx, Intent, Strategy};

pub struct JoinTheTouch {
    size: Qty,
}

impl Strategy for JoinTheTouch {
    fn name(&self) -> &'static str {
        "join_the_touch"
    }

    fn on_book(&mut self, ctx: &mut Ctx) {
        let (Some(bid), Some(ask)) = (ctx.book.best_bid(), ctx.book.best_ask()) else { return };
        if ctx.oms.open_orders() == 0 {
            ctx.intents.push(Intent::New { side: Side::Buy, price: bid.price, qty: self.size, tif: TimeInForce::PostOnly });
            ctx.intents.push(Intent::New { side: Side::Sell, price: ask.price, qty: self.size, tif: TimeInForce::PostOnly });
        }
    }
}
```

What `Ctx` gives you:

| Field | |
|---|---|
| `ctx.now` | Event time in ns. Use this, never the wall clock, so replays match live runs. |
| `ctx.book` | The L2 book: `best_bid()`, `best_ask()`, `mid()`, `microprice()`, `imbalance(n)`, `levels(side)` |
| `ctx.oms` | Our orders (`orders()`, with status), `position`, `open_buy_qty`, `open_sell_qty`, `pnl(mark)` |
| `ctx.intents` | Push `Intent::New { side, price, qty, tif }` or `Intent::Cancel { cl_id }` |

Other callbacks, all optional: `on_trade` (every public trade), `on_fill` (our
fills), and `diagnostics` / `diagnostic_names`, which publish up to eight
internal values to the console's Strategy tab.

Rules: don't allocate or block in callbacks. Prices are integer ticks and sizes
integer lots (`ctx.oms` and the book already are). A strategy never sends
anything itself; risk decides whether an intent becomes an order.

### Registering it

Add it to `crates/strategies/src/registry.rs`: a config struct with
`#[serde(deny_unknown_fields)]`, an arm in `build()` that converts sizes to
lots, and an entry in `BUILTIN`. Then `name = "join_the_touch"` works in any
config. For static dispatch in the hot loop, also add a variant to
`AnyStrategy`. Otherwise wrap it in `AnyStrategy::Dyn(Box::new(...))`, which
costs one virtual call per event.

## Writing a strategy in Python

With the [Python bindings](python.md), a class with an `on_book(ctx)` method
runs inside the same Rust event loop, with the same risk checks, OMS and fill
model:

```python
import tickrail

class JoinTheTouch:
    def on_book(self, ctx):
        if ctx.best_bid is None or ctx.open_orders:
            return
        ctx.buy(ctx.best_bid, 0.001)      # post-only by default
        ctx.sell(ctx.best_ask, 0.001)

    def on_fill(self, ctx, fill):         # optional
        order_id, side, price, size, maker = fill

result = tickrail.backtest("data/binance-btcusdt.journal", JoinTheTouch(),
                           max_order_size=0.002, max_position=0.01)
print(result["fills"], result["pnl"])
```

Python strategies run in backtests today. Calling Python adds a few
microseconds per event, which is fine for research but not for the live hot
path. Once a Python strategy works, port it to Rust.
