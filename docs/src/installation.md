# Installation and requirements

## Platforms

tickrail is plain Rust with no system dependencies beyond a C compiler for the
TLS library, so it builds anywhere Rust does. CI builds and tests on:

| OS | CPU | Notes |
|---|---|---|
| Linux (glibc) | x86_64, ARM64 | The production target. Supports core pinning. |
| macOS 11+ | Apple Silicon, Intel | Threads use QoS hints instead of pinning. |
| Windows 10+ | x86_64 | Works; no core pinning. |

Single-board computers (Raspberry Pi 4/5 on 64-bit OS) and small cloud
instances work with `engine.wait = "backoff"` or `"sleep:200us"`.

## Hardware

Measured on a live Binance BTCUSDT session (one symbol, console on):

| | Resident memory | CPU | Disk |
|---|---|---|---|
| `wait = "backoff"` | ~13 MB | ~5% of one core | 4 MB binary |
| `wait = "spin"` | ~14 MB | one full core per spinning thread | |
| Journal | | | ~80 MB per hour for a busy crypto pair |

So the practical requirements are:

- **Minimum:** 1 CPU core, 128 MB RAM, a network connection for live adapters.
- **Recommended for low-latency work:** Linux, 4+ physical cores (one each for
  the engine and feed, one for everything else), `wait = "spin"` with pinned,
  isolated cores. See [performance](performance.md).

## Build from source

Install Rust 1.88 or newer from [rustup.rs](https://rustup.rs), then:

```bash
git clone https://github.com/upendra-eth/tickrail
cd tickrail
cargo build --release
./target/release/tickrail --version
```

The binary is self-contained: the web console and default config are compiled
in. Put it on your `PATH` or run it from the repository so the example
configs in `config/` are at hand.

To build without some adapters, turn off default features:

```bash
cargo build --release -p tickrail-cli --no-default-features \
  --features tickrail-adapters/binance,tickrail-adapters/csv
```

## Docker

```bash
docker build -t tickrail .
docker run --rm -p 8080:8080 tickrail                         # simulator
docker run --rm -p 8080:8080 -v "$PWD/data:/app/data" tickrail \
  run -c config/binance.toml --set http.listen=0.0.0.0:8080
```

## Python bindings

```bash
pip install maturin
pip install ./bindings/python          # builds the Rust extension
python -c "import tickrail; print(tickrail.__version__)"
```

Or `make python` to build into a local `.venv`. See [Python](python.md).
