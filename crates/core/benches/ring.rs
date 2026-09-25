use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use tickrail_core::ring;

fn bench_ring(c: &mut Criterion) {
    let (mut tx, mut rx) = ring::channel::<[u64; 6]>(1024);
    c.bench_function("spsc push+pop same thread (48B msg)", |b| {
        b.iter(|| {
            tx.push(black_box([1, 2, 3, 4, 5, 6])).unwrap();
            black_box(rx.pop().unwrap());
        })
    });

    // Cross-core throughput: 1M messages through the ring to a spinning consumer.
    c.bench_function("spsc cross-thread 1M msgs", |b| {
        b.iter(|| {
            let (mut tx, mut rx) = ring::channel::<u64>(4096);
            let h = std::thread::spawn(move || {
                let mut n = 0u64;
                while n < 1_000_000 {
                    if rx.pop().is_some() {
                        n += 1;
                    }
                }
            });
            for i in 0..1_000_000u64 {
                tx.push_spin(i);
            }
            h.join().unwrap();
        })
    });
}

criterion_group!(benches, bench_ring);
criterion_main!(benches);
