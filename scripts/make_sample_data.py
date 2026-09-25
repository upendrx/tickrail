"""Generates examples/data/sample.csv: 60 s of a synthetic market in the csv
adapter's format. Deterministic (fixed seed) so tests and docs stay stable."""

import pathlib
import random

random.seed(7)
T0 = 1_714_000_000_000_000_000
STEP = 50_000_000  # 50 ms
out = pathlib.Path(__file__).resolve().parents[1] / "examples" / "data" / "sample.csv"


def snapshot(ts, mid):
    rows = [f"{ts},clear,,,"]
    for i in range(5):
        rows.append(f"{ts},level,B,{(mid - 1 - i) / 100:.2f},{random.randint(1, 40) * 10}")
        rows.append(f"{ts},level,S,{(mid + 1 + i) / 100:.2f},{random.randint(1, 40) * 10}")
    return rows


mid = 10_000  # cents
rows = ["ts_ns,event,side,price,qty"] + snapshot(T0, mid)
ts = T0
for _ in range(1200):
    ts += STEP
    r = random.random()
    if r < 0.3:
        mid += random.choice([-1, 1])
        rows += snapshot(ts, mid)
    else:
        side = random.choice("BS")
        # Most trades print at the touch; some sweep one or two levels deeper.
        depth = 1 + (random.random() < 0.25) + (random.random() < 0.08)
        px = (mid + depth if side == "B" else mid - depth) / 100
        rows.append(f"{ts},trade,{side},{px:.2f},{random.randint(1, 10) * 10}")

out.parent.mkdir(parents=True, exist_ok=True)
out.write_text("\n".join(rows) + "\n")
print(f"wrote {len(rows)} rows to {out}")
