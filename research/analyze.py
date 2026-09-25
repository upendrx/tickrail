"""Post-trade analysis of a session: fills, PnL and markouts.

    python research/analyze.py data/binance-btcusdt.journal     # needs `pip install ./bindings/python`
    python research/analyze.py data/session.csv                  # from `tickrail export`
    python research/analyze.py data/session.csv --plot out.png

A markout is how far the market moved after each fill, signed so that
positive is good for us. Consistently negative short-horizon markouts mean the
strategy is being adversely selected: it gets filled right before the price
moves against it. It's the first number to look at for any market maker.
"""

import argparse
from pathlib import Path

import polars as pl

HORIZONS_MS = [10, 100, 1000, 5000]


def load(path: Path):
    """Returns (trades, fills) frames with decimal prices and a `sign` column on fills."""
    if path.suffix == ".journal":
        import tickrail

        j = tickrail.read_journal(str(path))
        md = pl.DataFrame(j["md"])
        orders = pl.DataFrame(j["orders"])
        execs = pl.DataFrame(j["execs"])
        trades = md.filter(pl.col("event") == "trade").select("ts_ns", "price")
        sides = orders.filter(pl.col("cmd") == "new").select(pl.col("id"), pl.col("side"))
        fills = (
            execs.filter(pl.col("kind") == "fill")
            .join(sides, on="id", how="left")
            .with_columns(liquidity=pl.when(pl.col("maker")).then(pl.lit("Maker")).otherwise(pl.lit("Taker")))
            .select("ts_ns", "id", "side", "price", "qty", "liquidity")
        )
    else:
        df = pl.read_csv(path, schema_overrides={"side": pl.Utf8, "extra": pl.Utf8, "price": pl.Float64, "qty": pl.Float64})
        trades = df.filter((pl.col("record") == "md") & (pl.col("event") == "trade")).select("ts_ns", "price")
        sides = df.filter((pl.col("record") == "order") & (pl.col("event") == "new")).select("id", "side")
        fills = (
            df.filter((pl.col("record") == "exec") & (pl.col("event") == "fill"))
            .drop("side")
            .join(sides, on="id", how="left")
            .with_columns(liquidity=pl.col("extra").str.split("/").list.first())
            .select("ts_ns", "id", "side", "price", "qty", "liquidity")
        )
    fills = fills.sort("ts_ns").with_columns(sign=pl.when(pl.col("side") == "B").then(1.0).otherwise(-1.0))
    return trades.sort("ts_ns"), fills


def add_markouts(fills: pl.DataFrame, trades: pl.DataFrame) -> pl.DataFrame:
    ref = trades.rename({"ts_ns": "t", "price": "ref"})
    for h in HORIZONS_MS:
        future = fills.select((pl.col("ts_ns") + h * 1_000_000).alias("t")).join_asof(ref, on="t")
        fills = fills.with_columns(((future["ref"] - pl.col("price")) * pl.col("sign")).alias(f"mo_{h}ms"))
    return fills


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("path", type=Path)
    ap.add_argument("--plot", type=Path, help="save PnL and markout charts as PNG")
    a = ap.parse_args()

    trades, fills = load(a.path)
    print(f"{a.path}: {trades.height:,} public trades, {fills.height:,} of our fills")
    if fills.is_empty():
        return
    fills = fills.with_columns(
        cash=(-pl.col("sign") * pl.col("qty") * pl.col("price")).cum_sum(),
        position=(pl.col("sign") * pl.col("qty")).cum_sum(),
    ).with_columns(pnl=pl.col("cash") + pl.col("position") * pl.col("price"))
    fills = add_markouts(fills, trades)

    print(fills.group_by("liquidity", "side").agg(pl.len().alias("fills"), pl.col("qty").sum().alias("volume")).sort("side"))
    print("\nmean markout (price units, + = market moved our way after the fill):")
    print(fills.select([pl.col(f"mo_{h}ms").mean().alias(f"{h} ms") for h in HORIZONS_MS]))
    print(f"\nPnL marked at last fill price, before fees: {fills['pnl'][-1]:.4f}")

    if a.plot:
        import matplotlib.pyplot as plt

        fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12, 4))
        t0 = fills["ts_ns"][0]
        ax1.plot((fills["ts_ns"] - t0) / 1e9, fills["pnl"])
        ax1.set(title="Cumulative PnL (before fees)", xlabel="seconds")
        ax2.bar([f"{h} ms" for h in HORIZONS_MS], [fills[f"mo_{h}ms"].mean() for h in HORIZONS_MS])
        ax2.axhline(0, color="grey", lw=0.8)
        ax2.set(title="Mean markout")
        fig.tight_layout()
        fig.savefig(a.plot, dpi=120)
        print(f"saved {a.plot}")


if __name__ == "__main__":
    main()
