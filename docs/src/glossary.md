# Glossary

**Adverse selection:** being filled mostly by traders who know the price is
about to move against you.

**Aggressor:** the side of a trade that crossed the spread (the taker).

**Ask / offer:** a resting order to sell. The best ask is the lowest.

**Bid:** a resting order to buy. The best bid is the highest.

**Batch:** the set of messages from one venue frame, applied together before
the strategy runs (`LAST_IN_BATCH`).

**Colocation:** running servers in the exchange's own data centre.

**IOC:** immediate-or-cancel. Trade what's possible now and cancel the rest.

**Journal:** tickrail's binary record of every event in a session.

**Kill switch:** an emergency stop that cancels all orders and blocks new ones.

**L2 / L3:** an order book aggregated by price level, or listing every order.

**Lot:** the smallest quantity step.

**Maker / taker:** an order that rests in the book, or one that trades
immediately against it.

**Markout:** the price move after a fill, signed so positive is good for the
filled side.

**Microprice:** the mid weighted by the sizes at the top of the book.

**Mid:** (best bid + best ask) / 2.

**Paper execution:** simulated fills against real market data.

**p50 / p99:** median and 99th-percentile latency.

**Post-only:** an order that is rejected rather than allowed to take liquidity.

**Reservation price:** fair value shifted against current inventory.

**SPSC ring:** a lock-free single-producer, single-consumer queue.

**Spread:** best ask minus best bid.

**Tick:** the smallest price step.

**Tick-to-order:** time from receiving market data to sending an order.

**Venue:** an exchange, broker or any other destination for orders or source
of data.
