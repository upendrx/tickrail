//! Journal to CSV. Prices and sizes are written as decimals, so the files are
//! readable by anything (Polars, pandas, spreadsheets) without knowing tick sizes.

use anyhow::Result;
use std::io::Write;
use std::path::Path;
use tickrail_core::*;
use tickrail_journal::{JournalReader, Record};

pub fn run(journal: &Path, out: &Path, feed_only: bool) -> Result<()> {
    let reader = JournalReader::open(journal)?;
    let inst = reader.instrument.clone();
    let px = |p: Price| format!("{:.*}", inst.price_decimals as usize, inst.px(p));
    let qty = |q: Qty| format!("{:.*}", inst.qty_decimals as usize, inst.qty(q));
    let sd = |s: Side| if s == Side::Buy { "B" } else { "S" };
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut w = std::io::BufWriter::new(std::fs::File::create(out)?);
    if feed_only {
        writeln!(w, "ts_ns,event,side,price,qty")?;
    } else {
        writeln!(w, "record,ts_ns,id,event,side,price,qty,extra")?;
    }
    let mut n = 0u64;
    for r in reader {
        match r {
            Record::Md(m) => {
                let (ev, side, p, q) = match m.kind {
                    MdKind::Level { side, price, qty: q } => ("level", sd(side), px(price), qty(q)),
                    MdKind::Top { side, price, qty: q } => ("top", sd(side), px(price), qty(q)),
                    MdKind::Trade { aggressor, price, qty: q } => ("trade", sd(aggressor), px(price), qty(q)),
                    MdKind::Clear => ("clear", "", String::new(), String::new()),
                };
                if feed_only {
                    writeln!(w, "{},{ev},{side},{p},{q}", m.exch_ts)?;
                } else {
                    writeln!(w, "md,{},{},{ev},{side},{p},{q},{}", m.recv_ts, m.seq, m.flags)?;
                }
            }
            Record::Order(_) | Record::Exec(_) if feed_only => continue,
            Record::Order(o) => match o.cmd {
                OrderCmd::New { cl_id, side, price, qty: q, tif } => {
                    writeln!(w, "order,{},{cl_id},new,{},{},{},{tif:?}", o.ts, sd(side), px(price), qty(q))?
                }
                OrderCmd::Cancel { cl_id } => writeln!(w, "order,{},{cl_id},cancel,,,,", o.ts)?,
            },
            Record::Exec(e) => match e.kind {
                ExecKind::Ack => writeln!(w, "exec,{},{},ack,,,,", e.ts, e.cl_id)?,
                ExecKind::Fill { price, qty: q, leaves, liquidity } => writeln!(
                    w,
                    "exec,{},{},fill,,{},{},{liquidity:?}/{}",
                    e.ts,
                    e.cl_id,
                    px(price),
                    qty(q),
                    qty(leaves)
                )?,
                ExecKind::Canceled => writeln!(w, "exec,{},{},canceled,,,,", e.ts, e.cl_id)?,
                ExecKind::Rejected { reason } => writeln!(w, "exec,{},{},rejected,,,,{reason:?}", e.ts, e.cl_id)?,
            },
        }
        n += 1;
    }
    w.flush()?;
    println!(
        "wrote {n} rows to {} ({} {})",
        out.display(),
        inst.symbol,
        if feed_only { "feed format" } else { "all events" }
    );
    Ok(())
}
