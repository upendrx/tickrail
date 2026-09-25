//! Minimal RFC 3339 parser for venue timestamps ("2024-03-01T14:30:00.123456Z").
//! Only UTC ("Z") or numeric offsets; enough for exchange feeds without pulling in a date crate.

/// Nanoseconds since the Unix epoch, or `None` if the string is malformed.
pub fn rfc3339_ns(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut i = 19;
    let mut frac_ns: i64 = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let mut scale = 100_000_000;
        while let Some(c) = b.get(i).filter(|c| c.is_ascii_digit()) {
            frac_ns += (*c - b'0') as i64 * scale;
            scale /= 10;
            i += 1;
        }
    }
    let offset_s = match b.get(i)? {
        b'Z' | b'z' => 0,
        sign @ (b'+' | b'-') => {
            let oh = num(i + 1..i + 3)?;
            let om = num(i + 4..i + 6)?;
            let o = oh * 3600 + om * 60;
            if *sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset_s;
    u64::try_from(secs * 1_000_000_000 + frac_ns).ok()
}

#[cfg(test)]
mod tests {
    use super::rfc3339_ns;

    #[test]
    fn parses() {
        assert_eq!(rfc3339_ns("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_ns("2021-02-22T15:51:44.208Z"), Some(1_614_009_104_208_000_000));
        assert_eq!(rfc3339_ns("2023-09-25T07:49:37.708706Z"), Some(1_695_628_177_708_706_000));
        assert_eq!(rfc3339_ns("2021-02-22T10:51:44-05:00"), Some(1_614_009_104_000_000_000));
        assert_eq!(rfc3339_ns("not a time"), None);
    }
}
