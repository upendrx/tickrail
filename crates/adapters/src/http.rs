//! Blocking HTTP GET for start-up metadata (symbol precision and so on).
//! Never used on the data path.

use anyhow::Context;

/// GET with up to three attempts; some venue CDNs reset the odd connection.
pub fn get_json(url: &str) -> anyhow::Result<serde_json::Value> {
    let mut last = None;
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(300 * attempt));
        }
        let res = ureq::get(url)
            .header("User-Agent", concat!("tickrail/", env!("CARGO_PKG_VERSION")))
            .call()
            .and_then(|mut r| r.body_mut().read_to_string());
        match res {
            Ok(body) => return serde_json::from_str(&body).with_context(|| format!("parsing response from {url}")),
            Err(e) => last = Some(e),
        }
    }
    Err(last.expect("at least one attempt")).with_context(|| format!("GET {url}"))
}

/// Number of decimals in a step size: "0.01000000" is 2, "1" is 0, "0.5" is 1.
pub fn decimals_of(step: &str) -> u32 {
    match step.split_once('.') {
        Some((_, frac)) => frac.trim_end_matches('0').len() as u32,
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn decimals() {
        assert_eq!(super::decimals_of("0.00010000"), 4);
        assert_eq!(super::decimals_of("1.00000000"), 0);
        assert_eq!(super::decimals_of("0.1"), 1);
        assert_eq!(super::decimals_of("5"), 0);
    }
}
