# Performance and tuning

## Measured numbers

Release build (LTO, one codegen unit). Hardware: Apple M1 Pro, macOS. Numbers
from Linux servers are very welcome as pull requests.

| What | Result | How |
|---|---|---|
| SPSC ring push + pop, 48-byte message | 8.3 ns | `cargo bench --bench ring` |
| SPSC ring across cores | ~7.7 ns per message (≈130 M/s) | same |
| L2 book update, near-touch mix | 9.7 ns | `cargo bench --bench book` |
| Microprice | 1.3 ns | same |
| Matching engine operation | 86 ns | `cargo bench --bench matching` |
| Replay throughput | 10 to 14 M events/s | `tickrail replay` |
| Decision latency in replay (book → strategy → risk → send) | p50 ≈ 80 to 375 ns | same |
| Engine pass per message, live simulator | p50 42 ns | console, Latency tab |
| Tick-to-order across threads, `wait = "spin"` | p50 ≈ 300 to 500 ns | simulator session |

On Apple Silicon the system counter ticks every 41.7 ns, so small values are
quantised to that.

## Wait strategies

Every polling thread (engine, simulator) uses `engine.wait` when it finds no
work:

| Mode | Latency | CPU | Use it on |
|---|---|---|---|
| `spin` | lowest; the thread never sleeps | 100% of a core per thread | dedicated servers with isolated cores |
| `yield` | low | ~100%, but shares the core | machines where spinning threads compete |
| `backoff` (default) | spins ~20 µs, then yields, then sleeps 50 µs; adds up to ~50 µs after idle periods | a few % when quiet | laptops, desktops, VMs, CI |
| `sleep:<n>us` | adds up to `n` µs | minimal | Raspberry Pi, battery, very small instances |

Switching the same simulator session from `backoff` to `spin` moves median
tick-to-order from about 34 µs to about 0.5 µs on the reference machine.

## Linux tuning for low latency

On a dedicated box, in roughly the order they matter:

1. **Isolate cores** from the scheduler: boot with
   `isolcpus=2-5 nohz_full=2-5 rcu_nocbs=2-5`.
2. **Pin threads** to them: `engine.core = 3`, `simulator.core = 2`, and
   `wait = "spin"`.
3. **Fix the clock speed**: `cpupower frequency-set -g performance`. Disable
   deep C-states (`intel_idle.max_cstate=1` or `processor.max_cstate=1`), and
   consider disabling turbo for consistency.
4. **Keep interrupts off hot cores**: stop `irqbalance`, and steer NIC IRQs
   to housekeeping cores via `/proc/irq/*/smp_affinity`.
5. **SMT**: don't put a hot thread on a core whose sibling is busy. Disable SMT,
   or leave siblings idle.
6. **NUMA**: keep the process, its memory and the NIC on the same socket
   (`numactl --cpunodebind=0 --membind=0`).
7. **Real-time priority** for the hot threads if you can't isolate:
   `chrt -f 50`.

Beyond this the next steps are kernel bypass networking (ef_vi, DPDK,
AF_XDP), hardware timestamps with PTP, and colocation. See the
[roadmap](roadmap.md).

## Measuring

- `cargo bench` for component benchmarks (criterion, HTML reports in
  `target/criterion`).
- The Latency tab and session summary for end-to-end percentiles.
- `tickrail replay` for decision latency without network or scheduler noise.
- Profilers: `perf record -g` on Linux, Instruments or `samply` on macOS. The
  release profile keeps line tables for this.
