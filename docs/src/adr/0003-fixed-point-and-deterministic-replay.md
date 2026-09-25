# 0003 Fixed-point prices, event-time strategies, binary journal

**Status:** accepted

## Decision

- Prices are `i64` ticks and sizes `i64` lots. Venue strings and JSON numbers
  are parsed exactly (`parse_fixed`) and never pass through `f64`.
- Strategies read event time (`ctx.now`) only, never the wall clock.
- Every market-data message, order and execution is journaled as a fixed
  48-byte record, after a header that names the instrument.

## Consequences

- No rounding drift in PnL or order prices, and exact price comparisons.
- Replays reproduce a session exactly at 10M+ events per second, which is
  what makes parameter sweeps and regression tests practical.
- The journal doubles as an audit trail.
- Every adapter must know the instrument's precision, from the venue's
  metadata API or from config.
