# Contributing to tickrail

Thanks for taking the time. Bug reports, new venue adapters, strategies, docs
fixes and performance work are all welcome.

## Getting set up

You need Rust 1.88 or newer (`rustup update stable`). Python 3.9+ and
[maturin](https://www.maturin.rs/) are only needed if you touch the bindings.

```bash
git clone https://github.com/upendrx/tickrail && cd tickrail
cargo build --release
cargo test --workspace
./target/release/tickrail run          # built-in simulator, console on :8080
```

`make help` lists the common tasks (tests, lints, benchmarks, docs, Python).

## Branches

| Branch | Purpose |
|---|---|
| `main` | Stable. Every release is a tag on `main` (`v0.1.0`, ...). Only release and hotfix merges land here. |
| `develop` | Integration branch. Pull requests target `develop` unless they're hotfixes. |
| `feature/<short-name>` | New functionality, e.g. `feature/bybit-adapter` |
| `fix/<short-name>` | Bug fixes, e.g. `fix/kraken-reconnect` |
| `docs/<short-name>` | Documentation only |
| `perf/<short-name>` | Performance work, with benchmark numbers in the PR |
| `release/vX.Y` | Release preparation: version bumps, changelog, final fixes |
| `hotfix/<short-name>` | Urgent fixes branched from `main`, merged back to `main` and `develop` |

Use lowercase and hyphens, and keep one topic per branch. Branch from
`develop`, and rebase on it before opening the pull request.

## Before you open a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same three on every pull request and on pushes to `main`, `develop`
and `release/*`: on Linux (x86_64 and ARM64), macOS and Windows, plus
the Python tests. Keep pull requests focused: one adapter, one fix or one
feature at a time is much easier to review than a bundle.

## Where things live

| Path | What |
|---|---|
| `crates/core` | Types, events, rings, clock, wait strategies. Change with care: everything depends on it. |
| `crates/engine` | The event loop and the `Venue` trait. |
| `crates/adapters` | Market-data adapters, one file per venue. **The easiest place to start.** |
| `crates/strategies` | `Strategy` trait, built-ins, registry. |
| `crates/risk`, `crates/oms` | Pre-trade checks, order state and PnL. |
| `crates/sim` | Matching engine and simulated venue. |
| `crates/journal` | Binary event log. |
| `crates/telemetry` | Metrics and the web server. |
| `apps/cli` | The `tickrail` binary and config loading. |
| `bindings/python` | PyO3 bindings. |
| `ui/` | Web console and guide (plain HTML and JS, no build step). |
| `docs/` | The developer book (mdBook). |

The [developer docs](docs/src/SUMMARY.md) have step-by-step guides for
[adding an adapter](docs/src/adapters.md) and
[writing a strategy](docs/src/strategies.md).

## Ground rules for hot-path code

Anything the engine thread runs (book, strategy, risk, OMS, venue):

- no allocation after start-up, no locks, no syscalls, no I/O;
- prices and sizes stay integer ticks and lots;
- time comes from the event (`ctx.now`), never the wall clock, so replays match;
- if it's performance-sensitive, add or update a criterion benchmark and quote
  before/after numbers in the pull request.

Adapters run on their own thread and can allocate and parse freely, but they
must never block the engine. When the ring is full they drop and count.

## Adding a venue adapter

1. Copy the closest existing adapter in `crates/adapters/src/`. `okx.rs` is a
   compact WebSocket example and `csv.rs` a non-network one.
2. Add a cargo feature and a match arm in `crates/adapters/src/lib.rs`.
3. Add parser unit tests using real sample frames from the venue's docs.
4. Add `config/<venue>.toml` and a row to the adapter table in `docs/src/adapters.md`.
5. In the pull request, say how you tested it live (symbol, duration, any reconnects).

## Commit messages and changelog

Write commit subjects in the imperative ("Add Bybit adapter", "Fix cancel
reject handling"). User-visible changes get a line under `Unreleased` in
[CHANGELOG.md](CHANGELOG.md).

## Licensing

tickrail is dual-licensed under MIT or Apache-2.0. Unless you say otherwise,
any contribution you submit is licensed the same way, without additional terms.

## Conduct

Everyone taking part is expected to follow the [code of conduct](CODE_OF_CONDUCT.md).
