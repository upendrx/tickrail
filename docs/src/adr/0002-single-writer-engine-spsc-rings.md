# 0002 Single-writer engine connected by SPSC rings

**Status:** accepted

## Decision

All trading state (book, OMS, risk, strategy) is owned by one thread. Other
threads (feed, simulator, journal, telemetry) exchange `Copy` messages with it
through bounded lock-free single-producer, single-consumer rings.

## Why

- No locks and no shared mutable state: no contention, no priority inversion,
  no cache lines bouncing between cores.
- A single, deterministic order of events, which makes replay exact.
- A well-tested design: the LMAX Disruptor, Aeron IPC and "tile" architectures
  in modern trading and blockchain systems work the same way.

## Consequences

- One engine thread handles one group of instruments. Scaling means sharding
  instruments across engine threads or processes.
- Slow consumers must drop (telemetry, market data with gap detection) or
  apply back-pressure (execution reports). They must never block the engine.
