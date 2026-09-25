//! Append-only journal of every market-data message, order and execution.
//!
//! File layout: an 8-byte magic, a small header describing the instrument, then
//! fixed 48-byte little-endian records (SBE-style: no parsing, no allocation,
//! O(1) seek to record N). The engine thread pushes records into an SPSC ring
//! and a background thread does the file I/O, so disk latency never reaches the
//! hot path. The same file drives `replay`, which makes backtests reproduce
//! exactly what the engine saw live.

use anyhow::{Context, bail};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tickrail_core::ring::Consumer;
use tickrail_core::*;

pub const MAGIC: &[u8; 8] = b"TKRJ0002";
pub const RECORD_SIZE: usize = 48;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Md(MdMsg),
    Order(OrderMsg),
    Exec(ExecMsg),
}

fn put(buf: &mut [u8; RECORD_SIZE], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

fn get(buf: &[u8; RECORD_SIZE], off: usize) -> u64 {
    u64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}

/// Layout: [0]=type [1]=subtype [2]=side [3]=flags/tif/liq/reason [4..8]=instrument
/// [8..16]=ts [16..24]=seq/cl_id [24..32]=price [32..40]=qty [40..48]=exch_ts/leaves
pub fn encode(r: &Record, b: &mut [u8; RECORD_SIZE]) {
    *b = [0; RECORD_SIZE];
    match *r {
        Record::Md(m) => {
            b[0] = 1;
            b[3] = m.flags;
            b[4..8].copy_from_slice(&m.instrument.to_le_bytes());
            put(b, 8, m.recv_ts);
            put(b, 16, m.seq);
            put(b, 40, m.exch_ts);
            match m.kind {
                MdKind::Level { side, price, qty } => {
                    b[1] = 1;
                    b[2] = side as u8;
                    put(b, 24, price.0 as u64);
                    put(b, 32, qty.0 as u64);
                }
                MdKind::Trade { aggressor, price, qty } => {
                    b[1] = 2;
                    b[2] = aggressor as u8;
                    put(b, 24, price.0 as u64);
                    put(b, 32, qty.0 as u64);
                }
                MdKind::Clear => b[1] = 3,
                MdKind::Top { side, price, qty } => {
                    b[1] = 4;
                    b[2] = side as u8;
                    put(b, 24, price.0 as u64);
                    put(b, 32, qty.0 as u64);
                }
            }
        }
        Record::Order(o) => {
            b[0] = 2;
            b[4..8].copy_from_slice(&o.instrument.to_le_bytes());
            put(b, 8, o.ts);
            match o.cmd {
                OrderCmd::New { cl_id, side, price, qty, tif } => {
                    b[1] = 1;
                    b[2] = side as u8;
                    b[3] = tif as u8;
                    put(b, 16, cl_id);
                    put(b, 24, price.0 as u64);
                    put(b, 32, qty.0 as u64);
                }
                OrderCmd::Cancel { cl_id } => {
                    b[1] = 2;
                    put(b, 16, cl_id);
                }
            }
        }
        Record::Exec(e) => {
            b[0] = 3;
            b[4..8].copy_from_slice(&e.instrument.to_le_bytes());
            put(b, 8, e.ts);
            put(b, 16, e.cl_id);
            match e.kind {
                ExecKind::Ack => b[1] = 1,
                ExecKind::Fill { price, qty, leaves, liquidity } => {
                    b[1] = 2;
                    b[3] = liquidity as u8;
                    put(b, 24, price.0 as u64);
                    put(b, 32, qty.0 as u64);
                    put(b, 40, leaves.0 as u64);
                }
                ExecKind::Canceled => b[1] = 3,
                ExecKind::Rejected { reason } => {
                    b[1] = 4;
                    b[3] = reason as u8;
                }
            }
        }
    }
}

pub fn decode(b: &[u8; RECORD_SIZE]) -> Option<Record> {
    let instrument = u32::from_le_bytes(b[4..8].try_into().unwrap());
    let ts = get(b, 8);
    let price = Price(get(b, 24) as i64);
    let qty = Qty(get(b, 32) as i64);
    Some(match b[0] {
        1 => {
            let kind = match b[1] {
                1 => MdKind::Level { side: Side::from_u8(b[2])?, price, qty },
                2 => MdKind::Trade { aggressor: Side::from_u8(b[2])?, price, qty },
                3 => MdKind::Clear,
                4 => MdKind::Top { side: Side::from_u8(b[2])?, price, qty },
                _ => return None,
            };
            Record::Md(MdMsg { seq: get(b, 16), exch_ts: get(b, 40), recv_ts: ts, instrument, flags: b[3], kind })
        }
        2 => {
            let cl_id = get(b, 16);
            let cmd = match b[1] {
                1 => {
                    let tif = match b[3] {
                        0 => TimeInForce::Gtc,
                        1 => TimeInForce::Ioc,
                        2 => TimeInForce::PostOnly,
                        _ => return None,
                    };
                    OrderCmd::New { cl_id, side: Side::from_u8(b[2])?, price, qty, tif }
                }
                2 => OrderCmd::Cancel { cl_id },
                _ => return None,
            };
            Record::Order(OrderMsg { ts, instrument, cmd })
        }
        3 => {
            let kind = match b[1] {
                1 => ExecKind::Ack,
                2 => ExecKind::Fill {
                    price,
                    qty,
                    leaves: Qty(get(b, 40) as i64),
                    liquidity: if b[3] == 0 { Liquidity::Maker } else { Liquidity::Taker },
                },
                3 => ExecKind::Canceled,
                4 => ExecKind::Rejected { reason: RejectReason::from_u8(b[3])? },
                _ => return None,
            };
            Record::Exec(ExecMsg { ts, instrument, cl_id: get(b, 16), kind })
        }
        _ => return None,
    })
}

pub struct JournalWriter {
    out: BufWriter<File>,
    pub records: u64,
}

impl JournalWriter {
    pub fn create(path: &Path, inst: &Instrument) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let f = File::create(path).with_context(|| format!("create journal {}", path.display()))?;
        let mut out = BufWriter::with_capacity(1 << 20, f);
        out.write_all(MAGIC)?;
        // Header: price decimals, qty decimals, symbol length, symbol bytes.
        let sym = inst.symbol.as_bytes();
        let len = sym.len().min(u16::MAX as usize);
        out.write_all(&inst.price_decimals.to_le_bytes())?;
        out.write_all(&inst.qty_decimals.to_le_bytes())?;
        out.write_all(&(len as u16).to_le_bytes())?;
        out.write_all(&sym[..len])?;
        Ok(JournalWriter { out, records: 0 })
    }

    pub fn write(&mut self, r: &Record) -> std::io::Result<()> {
        let mut b = [0u8; RECORD_SIZE];
        encode(r, &mut b);
        self.records += 1;
        self.out.write_all(&b)
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

pub struct JournalReader {
    inp: BufReader<File>,
    /// Instrument the journal was recorded for.
    pub instrument: Instrument,
}

impl JournalReader {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let f = File::open(path).with_context(|| format!("open journal {}", path.display()))?;
        let mut inp = BufReader::with_capacity(1 << 20, f);
        let mut magic = [0u8; 8];
        inp.read_exact(&mut magic)?;
        if &magic != MAGIC {
            bail!("{} is not a journal file (or was written by an incompatible version)", path.display());
        }
        let mut u32b = [0u8; 4];
        inp.read_exact(&mut u32b)?;
        let price_decimals = u32::from_le_bytes(u32b);
        inp.read_exact(&mut u32b)?;
        let qty_decimals = u32::from_le_bytes(u32b);
        let mut u16b = [0u8; 2];
        inp.read_exact(&mut u16b)?;
        let mut sym = vec![0u8; u16::from_le_bytes(u16b) as usize];
        inp.read_exact(&mut sym)?;
        let instrument =
            Instrument { id: 1, symbol: String::from_utf8_lossy(&sym).into_owned(), price_decimals, qty_decimals };
        Ok(JournalReader { inp, instrument })
    }
}

impl Iterator for JournalReader {
    type Item = Record;
    fn next(&mut self) -> Option<Record> {
        let mut b = [0u8; RECORD_SIZE];
        loop {
            self.inp.read_exact(&mut b).ok()?;
            if let Some(r) = decode(&b) {
                return Some(r);
            }
        }
    }
}

/// Journal thread body: drains the ring to disk until `stop`, then drains the rest.
pub fn run_writer(mut rx: Consumer<Record>, mut w: JournalWriter, stop: Arc<AtomicBool>) -> u64 {
    cpu::configure_current_thread(cpu::ThreadRole::Background, None);
    loop {
        let mut n = 0;
        while let Some(r) = rx.pop() {
            if w.write(&r).is_err() {
                return w.records;
            }
            n += 1;
        }
        if n == 0 {
            if stop.load(Ordering::Relaxed) && rx.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    let _ = w.flush();
    w.records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_every_variant() {
        let recs = [
            Record::Md(MdMsg {
                seq: 7,
                exch_ts: 11,
                recv_ts: 12,
                instrument: 3,
                flags: md_flags::LAST_IN_BATCH,
                kind: MdKind::Level { side: Side::Sell, price: Price(-5), qty: Qty(9) },
            }),
            Record::Md(MdMsg { seq: 8, exch_ts: 1, recv_ts: 2, instrument: 3, flags: 0, kind: MdKind::Clear }),
            Record::Md(MdMsg {
                seq: 10,
                exch_ts: 1,
                recv_ts: 2,
                instrument: 3,
                flags: 0,
                kind: MdKind::Top { side: Side::Buy, price: Price(99), qty: Qty(4) },
            }),
            Record::Md(MdMsg {
                seq: 9,
                exch_ts: 1,
                recv_ts: 2,
                instrument: 3,
                flags: 0,
                kind: MdKind::Trade { aggressor: Side::Buy, price: Price(100), qty: Qty(1) },
            }),
            Record::Order(OrderMsg {
                ts: 5,
                instrument: 3,
                cmd: OrderCmd::New {
                    cl_id: 77,
                    side: Side::Buy,
                    price: Price(10),
                    qty: Qty(2),
                    tif: TimeInForce::PostOnly,
                },
            }),
            Record::Order(OrderMsg { ts: 5, instrument: 3, cmd: OrderCmd::Cancel { cl_id: 77 } }),
            Record::Exec(ExecMsg {
                ts: 6,
                instrument: 3,
                cl_id: 77,
                kind: ExecKind::Fill { price: Price(10), qty: Qty(1), leaves: Qty(1), liquidity: Liquidity::Taker },
            }),
            Record::Exec(ExecMsg {
                ts: 6,
                instrument: 3,
                cl_id: 77,
                kind: ExecKind::Rejected { reason: RejectReason::RiskPriceBand },
            }),
            Record::Exec(ExecMsg { ts: 6, instrument: 3, cl_id: 77, kind: ExecKind::Ack }),
        ];
        let mut b = [0u8; RECORD_SIZE];
        for r in recs {
            encode(&r, &mut b);
            assert_eq!(decode(&b), Some(r));
        }
    }

    #[test]
    fn file_roundtrip_keeps_instrument() {
        let path = std::env::temp_dir().join(format!("journal-test-{}.journal", std::process::id()));
        let inst = Instrument { id: 1, symbol: "ETH-USDT".into(), price_decimals: 2, qty_decimals: 6 };
        let mut w = JournalWriter::create(&path, &inst).unwrap();
        let rec = Record::Md(MdMsg { seq: 1, exch_ts: 2, recv_ts: 3, instrument: 1, flags: 1, kind: MdKind::Clear });
        w.write(&rec).unwrap();
        w.flush().unwrap();
        let r = JournalReader::open(&path).unwrap();
        assert_eq!(r.instrument.symbol, "ETH-USDT");
        assert_eq!(r.instrument.qty_decimals, 6);
        assert_eq!(r.collect::<Vec<_>>(), vec![rec]);
        let _ = std::fs::remove_file(path);
    }
}
