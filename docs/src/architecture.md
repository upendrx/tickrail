# Architecture

## Threads and rings

```text
             ┌──────────────┐  MdMsg ring   ┌──────────────────────────────────────────┐
 venue ────▶ │ feed adapter │ ────────────▶ │ engine thread                            │
 (WS/file)   │   thread     │               │  book → strategy → risk → OMS → venue    │
             └──────────────┘               │                               │  ▲       │
                                            └───────┬───────────────┬───────┼──┼───────┘
     or, with the simulator:                        │ journal ring  │ stats │  │ execs
             ┌──────────────┐  orders ring          ▼               ▼ rings │  │
             │  simulator   │ ◀─────────────── (venue = SimGateway) ────────┘  │
             │   thread     │ ─────────────────────────────────────────────────┘
             └──────────────┘               journal thread     telemetry (tokio)
                                             (disk I/O)         HTTP + WebSocket ──▶ browser
```

- **One writer.** Only the engine thread changes trading state: the book, OMS,
  risk counters and strategy. Nothing on it locks.
- **SPSC rings** (`tickrail_core::ring`) are the only way threads communicate.
  Producer and consumer indices sit on separate 128-byte-aligned cache lines,
  and each side caches the other's index, so the shared line is only touched
  when the ring looks full or empty.
- **Back-pressure rules.** Market data is dropped (and counted as a sequence
  gap) if the engine falls behind, because a feed must never stall. Execution
  reports are never dropped: the producer waits. Telemetry and journal records
  are dropped rather than slow the engine.
- **Slow things live elsewhere.** TLS, JSON, disk, histograms and HTTP all run
  on other threads.

## One event, step by step

For each `MdMsg` popped from the ring, `Engine::on_md`:

1. updates gap and latency counters and journals the message;
2. applies it to the L2 book (a trade also calls `strategy.on_trade`);
3. lets the venue see it (the paper venue decides fills here);
4. drains execution reports into the OMS, calling `strategy.on_fill`;
5. if the message closes a batch (`LAST_IN_BATCH`) and the book isn't crossed,
   calls `strategy.on_book`;
6. runs each intent through risk, assigns an id, records it in the OMS and
   sends it to the venue.

A housekeeping pass every 1,024 loop iterations handles the kill switch, the
loss limit and the 100 ms stats snapshot.

## Message types

All `Copy`, fixed size, no heap (`tickrail_core::events`):

- `MdMsg { seq, exch_ts, recv_ts, instrument, flags, kind }`, where `kind` is
  one of `Level`, `Top`, `Trade` or `Clear`;
- `OrderMsg { ts, instrument, cmd }`, where `cmd` is `New` or `Cancel`;
- `ExecMsg { ts, instrument, cl_id, kind }`, where `kind` is `Ack`, `Fill`,
  `Canceled` or `Rejected`.

## Extension points

| To add | Implement | Where |
|---|---|---|
| A market-data source | `FeedAdapter` (or `ws::WsProtocol` for WebSocket JSON) | `crates/adapters` |
| An order destination | `Venue` (`send`, `on_market`, `poll`) | `crates/engine/src/venue.rs` |
| A strategy | `Strategy` | `crates/strategies` |
| A UI | read `/ws` or `/api/snapshot` | anywhere |

`Engine<S: Strategy, V: Venue>` is generic over both, so the same engine runs
the simulator, paper execution and replay. A real order gateway would be one
more `Venue`.

## Crates

| Crate | Depends on | |
|---|---|---|
| `tickrail-core` | none | types, events, ring, clock, CPU placement, wait strategies, RNG |
| `tickrail-book` | core | L2 book |
| `tickrail-oms` | core | order state, position, PnL |
| `tickrail-risk` | core, oms | pre-trade checks, kill switch |
| `tickrail-strategies` | core, book, oms | trait, built-ins, registry |
| `tickrail-journal` | core | binary journal |
| `tickrail-engine` | all of the above | event loop, venues, stats types |
| `tickrail-sim` | core | matching engine, simulated venue |
| `tickrail-adapters` | core | market-data adapters |
| `tickrail-telemetry` | core, engine | metrics, HTTP, UI |
| `tickrail-cli` | everything | the `tickrail` binary |
| `bindings/python` | engine and friends | PyO3 module |

## Platform-specific code

All of it is in `tickrail_core::{clock, cpu}`:

- **Clock:** `mach_absolute_time` on macOS, `Instant` elsewhere.
- **Thread placement:** `sched_setaffinity` on Linux when a core is
  configured; `QOS_CLASS_USER_INTERACTIVE` (hot threads) and `UTILITY`
  (background threads) on macOS; a no-op on other systems.
