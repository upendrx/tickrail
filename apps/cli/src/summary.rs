//! End-of-session report printed to stdout.

use tickrail_core::{Instrument, Qty};
use tickrail_engine::stats::EngineStats;
use tickrail_telemetry::Snapshot;

pub fn print(inst: &Instrument, s: &EngineStats, snap: Option<&Snapshot>, journaled: Option<u64>) {
    let q = |lots: i64| format!("{:.*}", inst.qty_decimals as usize, inst.qty(Qty(lots)));
    println!("\nsession summary: {}", inst.symbol);
    println!("  market data     {:>12}   gaps {}", s.md_msgs, s.md_gaps);
    println!("  orders/cancels  {:>12} / {}", s.orders_sent, s.cancels_sent);
    let maker = if s.fills > 0 { 100.0 * s.maker_fills as f64 / s.fills as f64 } else { 0.0 };
    println!("  fills           {:>12}   maker {maker:.0}%   volume {}", s.fills, q(s.volume_lots));
    println!("  rejects         risk {}   venue {}", s.risk_rejects, s.venue_rejects);
    println!("  position        {:>12}", q(s.position_lots));
    println!("  pnl (net)       {:>12.4}   fees {:.4}", s.pnl, s.fees);
    if let Some(sn) = snap {
        let l = &sn.tick_to_order;
        if l.count > 0 {
            println!(
                "  tick-to-order   p50 {} ns  p90 {} ns  p99 {} ns  p99.9 {} ns  ({} orders)",
                l.p50, l.p90, l.p99, l.p999, l.count
            );
        }
        let m = &sn.md_process;
        if m.count > 0 {
            println!("  engine pass     p50 {} ns  p99 {} ns", m.p50, m.p99);
        }
    }
    if let Some(n) = journaled {
        println!("  journal         {n} records (dropped {})", s.journal_dropped);
    }
}
