use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use tickrail_book::L2Book;
use tickrail_core::{Price, Qty, Side, rng::Rng};

fn bench_book(c: &mut Criterion) {
    let mut book = L2Book::with_capacity(512);
    for i in 0..200 {
        book.set_level(Side::Buy, Price(10_000 - i), Qty(10));
        book.set_level(Side::Sell, Price(10_001 + i), Qty(10));
    }
    let mut rng = Rng::new(7);
    // Realistic mix: updates concentrated near the touch.
    let updates: Vec<(Side, i64, i64)> = (0..4096)
        .map(|_| {
            let side = if rng.coin() { Side::Buy } else { Side::Sell };
            let dist = rng.geometric(0.35) as i64;
            let px = match side {
                Side::Buy => 10_000 - dist,
                Side::Sell => 10_001 + dist,
            };
            (side, px, rng.below(20) as i64 + 1)
        })
        .collect();
    let mut i = 0;
    c.bench_function("l2 book update (near-touch mix)", |b| {
        b.iter(|| {
            let (s, p, q) = updates[i & 4095];
            i += 1;
            book.set_level(s, Price(p), Qty(q));
            black_box(book.best_bid());
        })
    });
    c.bench_function("l2 book microprice", |b| b.iter(|| black_box(book.microprice())));
}

criterion_group!(benches, bench_book);
criterion_main!(benches);
