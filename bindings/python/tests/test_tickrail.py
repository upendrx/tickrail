import math
import pathlib

import tickrail

ROOT = pathlib.Path(__file__).resolve().parents[3]
SAMPLE = str(ROOT / "examples" / "data" / "sample.csv")
CSV_ARGS = dict(max_order_size=20, max_position=200, price_decimals=2, qty_decimals=0, symbol="SAMPLE")


def test_order_book():
    b = tickrail.OrderBook(price_decimals=2, qty_decimals=3)
    b.set_level("B", 100.00, 5.0)
    b.set_level("B", 99.99, 1.0)
    b.set_level("S", 100.02, 1.0)
    assert b.best_bid() == (100.0, 5.0)
    assert b.best_ask() == (100.02, 1.0)
    assert math.isclose(b.spread(), 0.02)
    assert b.microprice() > b.mid()  # more size on the bid: leans up
    assert b.levels("B", 5) == [(100.0, 5.0), (99.99, 1.0)]
    b.set_level("B", 100.00, 0)
    assert b.best_bid() == (99.99, 1.0)


def test_builtin_backtest():
    r = tickrail.backtest(SAMPLE, "market_maker", params={"size": 10, "max_inventory": 100, "half_spread_ticks": 1}, **CSV_ARGS)
    assert r["events"] > 4000
    assert r["orders"] > 0
    assert len(r["fill_log"]["price"]) == r["fills"]
    assert len(r["equity_curve"]["pnl"]) > 1


def test_python_strategy_runs_through_risk():
    class Greedy:
        calls = 0

        def on_book(self, ctx):
            Greedy.calls += 1
            if ctx.best_bid and not ctx.open_orders:
                ctx.buy(ctx.best_bid, 1_000)  # far above max_order_size: risk must reject

    r = tickrail.backtest(SAMPLE, Greedy(), **CSV_ARGS)
    assert Greedy.calls > 100
    assert r["orders"] == 0
    assert r["risk_rejects"] > 0


def test_python_errors_propagate():
    class Broken:
        def on_book(self, ctx):
            raise ValueError("boom")

    try:
        tickrail.backtest(SAMPLE, Broken(), **CSV_ARGS)
    except ValueError as e:
        assert "boom" in str(e)
    else:
        raise AssertionError("expected the strategy's exception")


def test_read_journal(tmp_path):
    import subprocess

    exe = ROOT / "target" / "release" / "tickrail"
    if not exe.exists():
        return
    j = tmp_path / "t.journal"
    subprocess.run([str(exe), "run", "--set", "engine.duration_secs=1", "--set", "http.enabled=false",
                    "--set", f"engine.journal={j}"], check=True, capture_output=True)
    d = tickrail.read_journal(str(j))
    assert d["symbol"] == "SIM"
    assert len(d["md"]["ts_ns"]) == len(d["md"]["price"]) > 100
