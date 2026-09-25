# Exchange simulator

`adapter = "simulator"` replaces the network with a local exchange on its own
thread: a price-time-priority matching engine, background order flow, and a
latency model for our orders. It needs no network and is deterministic for a
given seed.

## Matching engine

`tickrail_sim::MatchingEngine` is a standard central limit order book:

- price-time priority (best price first, then first-come at each price);
- GTC, IOC and post-only orders; partial fills; cancels;
- public output (level changes, trades) and private output (acks, fills,
  cancels, rejects) as `MatchEvent`s.

It's simulation-grade (a `BTreeMap` of levels, each a `VecDeque` of orders) at
about 86 ns per operation, and is also useful on its own for testing.

## Background flow

A hidden **fair value** does a Gaussian random walk (`volatility`, in ticks
per √s). Synthetic traders generate `event_rate` events per second:

- about 52%: passive limit orders a geometric distance from the fair value;
- about 40%: cancels of random resting orders;
- about 8%: aggressive IOC orders, biased toward the side the fair value has
  moved to.

Because background orders are placed around the *hidden* value, a quote that
lags it gets picked off. The simulator therefore has real adverse selection,
and a naive market maker loses money here, as it would in a real market.

## Order latency

Our orders reach the matching engine after `latency_us`, one way. Sweeping it
shows how speed interacts with adverse selection:

```bash
for us in 1 20 200 2000; do
  tickrail run --set simulator.latency_us=$us --set engine.duration_secs=10 --set http.enabled=false
done
```
