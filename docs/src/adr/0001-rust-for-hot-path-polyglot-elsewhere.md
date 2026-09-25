# 0001 Rust on the hot path, the right tool elsewhere

**Status:** accepted

## Context

Trading firms broadly split three ways. C++ plus FPGA (most of the large
market makers), one typed functional language everywhere (Jane Street, OCaml),
and ML-first shops that put Python and C++ on GPU clusters. The hot path needs
predictable latency with no garbage-collection pauses. Research needs fast
iteration and a large numeric ecosystem.

## Decision

- **Rust** for everything between a market event and an order: adapters,
  book, strategy, risk, OMS, venues, journal, simulator.
- **Python** (with Polars) for research and analysis, through bindings to the
  same Rust code.
- **HTML and JavaScript** for the console. HDL for FPGA work, if it ever
  happens. C only as FFI to vendor network SDKs.

## Consequences

- C++-class speed (single-digit-nanosecond book updates) without use-after-free
  bugs or data races on the path that sends orders.
- Researchers prototype in Python against the real engine, and port what works.
- The contributor pool is smaller than C++'s, but growing in trading.
- Some vendor SDKs are C or C++ only and need FFI wrappers.
