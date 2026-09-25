use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use tickrail_core::{Price, Qty, Side, TimeInForce, rng::Rng};
use tickrail_sim::{MatchingEngine, NewOrder};

fn bench_matching(c: &mut Criterion) {
    let mut m = MatchingEngine::new();
    let mut ev = Vec::with_capacity(1024);
    let mut rng = Rng::new(1);
    let mut id = 0u64;
    let mut live: Vec<u64> = Vec::new();
    c.bench_function("matching engine: mixed add/cancel/cross", |b| {
        b.iter(|| {
            ev.clear();
            id += 1;
            let r = rng.f64();
            if (r < 0.5 && live.len() < 2_000) || live.is_empty() {
                let side = if rng.coin() { Side::Buy } else { Side::Sell };
                let d = rng.geometric(0.3) as i64;
                let px = if side == Side::Buy { 10_000 - d } else { 10_001 + d };
                m.submit(
                    NewOrder { owner: 1, id, side, price: Price(px), qty: Qty(5), tif: TimeInForce::Gtc },
                    &mut ev,
                );
                live.push(id);
            } else if r < 0.9 {
                let i = rng.below(live.len() as u64) as usize;
                let oid = live.swap_remove(i);
                m.cancel(1, oid, &mut ev);
            } else {
                let side = if rng.coin() { Side::Buy } else { Side::Sell };
                let px = if side == Side::Buy { 10_010 } else { 9_990 };
                m.submit(
                    NewOrder { owner: 1, id, side, price: Price(px), qty: Qty(8), tif: TimeInForce::Ioc },
                    &mut ev,
                );
            }
            black_box(&ev);
        })
    });
}

criterion_group!(benches, bench_matching);
criterion_main!(benches);
