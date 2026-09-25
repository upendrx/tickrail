## What and why

<!-- One or two sentences. Link the issue if there is one. -->

## How it was tested

<!-- Unit tests, a live run (venue, symbol, duration), a replay, benchmarks. -->

## Checklist

- [ ] `cargo fmt`, `cargo clippy -- -D warnings` and `cargo test` pass
- [ ] Hot-path changes: no new allocation, locks or syscalls on the engine thread
- [ ] Performance-sensitive changes include before/after benchmark numbers
- [ ] Docs and `CHANGELOG.md` updated for user-visible changes
