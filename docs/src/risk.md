# Risk and order management

## Pre-trade checks

Every new order goes through `RiskEngine::check_new` on the engine thread
before it reaches the venue. The checks, in order:

1. **Kill switch** off.
2. **Order size** within `max_order_size`.
3. **Open orders** under `max_open_orders`.
4. **Worst-case position** within `max_position`: current position, plus every
   open order on the same side, plus this order, as if all of them filled.
5. **Reference price exists** (the book has both sides).
6. **Price band**: within `price_band_ticks` of the mid. This catches bad
   prices from bugs and fat fingers.
7. **Rate limit**: under `max_orders_per_sec` in the current one-second
   window, measured in event time so it behaves the same in replay.

A rejected intent is counted by reason (see the console's Risk tab) and never
reaches the venue. **Cancels are never checked.** Blocking a cancel could trap
you in a position.

After every fill, and on each housekeeping pass, PnL is compared with
`max_loss`. Crossing it trips the kill switch.

## Kill switch

The kill switch is a single atomic flag shared between the engine and the web
server. When it's set, the engine cancels every working order on its next
housekeeping pass (within microseconds) and risk rejects every new order.
It can be tripped from the console, with `POST /api/kill`, or automatically
by the loss limit. `POST /api/unkill` clears it.

## Order management (OMS)

The OMS is the engine's record of its own orders and position. It is updated
synchronously on the engine thread.

```text
            send new                ack                     fill (leaves = 0)
  (none) ───────────▶ PendingNew ───────▶ Live ─────────────────────────────▶ (gone)
                          │                 │  send cancel          canceled
                          │                 └──────────────▶ PendingCancel ──────▶ (gone)
                          │ reject (never acked)                  │ cancel reject
                          └──────────────▶ (gone)                 └──▶ Live, or gone if
                                                                       the venue no longer has it
```

A reject for an order that was never acknowledged is a rejected *new* order,
even if a cancel for it was already sent. A cancel reject with "unknown order"
means the order already finished at the venue, so it's removed. Both cases
are covered by tests, since getting them wrong leaves ghost orders that the
strategy then keeps trying to cancel.

Position is in lots. Cash is kept in integer tick×lot units, so PnL has no
rounding drift. Fees are charged per fill from the `[execution]` fee schedule.
