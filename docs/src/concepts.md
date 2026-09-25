# Market concepts

This chapter explains the vocabulary the engine and console use. None of it
is specific to tickrail; it's how electronic markets work.

## The order book

A venue keeps a list of everyone waiting to trade. **Bids** are offers to buy
("up to 2 BTC at 84,210.00"), **asks** (or offers) are offers to sell. The
highest bid and lowest ask are the **top of book**, and the gap between them
is the **spread**.

An **L2** book aggregates size per price level; an L3 book lists every order.
tickrail keeps L2, because that's what public feeds provide.

Prices move in **ticks**, the smallest allowed step (0.01 for BTCUSDT on
Binance, $0.01 for most US stocks), and sizes in **lots**. The engine stores
both as integers: `84210.00` with a 0.01 tick is `8,421,000` ticks. See
[fixed-point and replay](adr/0003-fixed-point-and-deterministic-replay.md)
for why.

## Makers and takers

An order that rests in the book adds liquidity and is a **maker**. An order
that trades immediately against resting orders removes it and is a
**taker**. The taker is also called the **aggressor**, and trade feeds report
which side it was. Venues usually charge takers more than makers, and some
pay makers a rebate.

A **post-only** order is rejected instead of trading on arrival, which
guarantees maker status. An **IOC** (immediate-or-cancel) order trades what
it can at once and cancels the rest.

## Fair value

The **mid** is `(best bid + best ask) / 2`. The **microprice** weights each
side by the size on the *other* side:

```text
microprice = (bid × ask_size + ask × bid_size) / (bid_size + ask_size)
```

When there's much more size bid than offered, the ask is more likely to be
taken out next, so the microprice sits closer to the ask. It's a better
one-tick-ahead predictor than the mid, and it's the fair value the built-in
market maker uses. **Order-book imbalance**, `(bid size - ask size) / total`,
measures the same pressure over several levels.

## Market making

A market maker keeps a bid just below fair value and an ask just above it. If
both fill, it earns the gap. The risks are:

- **Inventory.** If only one side fills, you hold a position that can move
  against you. The built-in strategy shifts both quotes away from its position
  (the **reservation price**, from Avellaneda and Stoikov, 2008) so trades tend
  to flatten it.
- **Adverse selection.** The traders most eager to hit your quote are often the
  ones who know the price is about to move. You get filled right before the
  market goes against you.
- **Latency.** The slower you are to move a quote after news, the more often a
  faster trader takes it at a stale price.

## Markouts

A **markout** is how far the price moved after a fill, signed so positive is
good for you:

```text
markout(h) = side × (mid at fill time + h − fill price)      side = +1 buy, −1 sell
```

Averaged over many fills at horizons from milliseconds to seconds, markouts
tell you whether your fills are good. Consistently negative short-horizon
markouts are adverse selection, and it's the first number a trading desk
looks at. `research/analyze.py` and the console's Orders tab both show them.

## Latency

**Tick-to-order** is the time from receiving market data to sending an order.
In tickrail it's measured from the moment the feed thread received the
message to the moment the engine hands the order to the venue, so it includes
the hop between threads, the book update, the strategy and risk.

Always look at the **distribution**, not the average: p50 (median), p99 and
p99.9. The slow tail usually happens in the busiest moments, which are the
ones that matter most. For reference, published FPGA tick-to-trade records
are around 14 ns, and tuned software on isolated Linux cores is typically in
the low microseconds. See [performance](performance.md) for what tickrail
measures.

## Paper execution

Without a real order gateway, fills have to be simulated. tickrail's paper
venue is deliberately pessimistic about queue position: a resting order fills
only when a real trade prints *through* its price, or the opposite side of
the book crosses it. It is optimistic in other ways, since it assumes zero
order latency and no market impact. Treat paper PnL as a rough estimate and
markouts as the more reliable signal.
