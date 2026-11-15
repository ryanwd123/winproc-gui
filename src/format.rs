//! Number formatting for cells and summary tiles.

pub fn bytes(b: u64) -> String {
    let b = b as f64;
    const K: f64 = 1024.0;
    if b < K {
        format!("{b:.0} B")
    } else if b < K * K {
        format!("{:.0} KB", b / K)
    } else if b < K * K * K {
        let m = b / (K * K);
        if m < 100.0 { format!("{m:.1} MB") } else { format!("{m:.0} MB") }
    } else {
        format!("{:.2} GB", b / (K * K * K))
    }
}

/// Bytes per second.
pub fn rate(b: f64) -> String {
    if b < 1.0 {
        return "0".into();
    }
    const K: f64 = 1024.0;
    if b < K {
        format!("{b:.0} B/s")
    } else if b < K * K {
        format!("{:.0} KB/s", b / K)
    } else if b < K * K * K {
        let m = b / (K * K);
        if m < 100.0 { format!("{m:.1} MB/s") } else { format!("{m:.0} MB/s") }
    } else {
        format!("{:.2} GB/s", b / (K * K * K))
    }
}

/// Bits per second, for network link speeds and traffic.
pub fn bits(bps: f64) -> String {
    if bps >= 1e9 {
        format!("{:.1} Gbps", bps / 1e9)
    } else if bps >= 1e6 {
        format!("{:.1} Mbps", bps / 1e6)
    } else if bps >= 1e3 {
        format!("{:.0} Kbps", bps / 1e3)
    } else {
        format!("{bps:.0} bps")
    }
}

pub fn percent(p: f32) -> String {
    if p < 0.05 {
        "0".into()
    } else if p >= 99.95 {
        format!("{p:.0}")
    } else {
        format!("{p:.1}")
    }
}

/// 1234567 -> "1,234,567".
pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A span of time in 100 ns units: "12.3s", "4m 05s", "3h 12m", "2d 4h".
pub fn duration_100ns(t: u64) -> String {
    let secs = t / 10_000_000;
    if secs < 60 {
        format!("{:.1}s", t as f64 / 1e7)
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{}h {:02}m", secs / 3600, (secs / 60) % 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs / 3600) % 24)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(count(1234567), "1,234,567");
        assert_eq!(count(12), "12");
        assert_eq!(bytes(1536), "2 KB");
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(duration_100ns(125 * 10_000_000), "2m 05s");
        assert_eq!(percent(0.01), "0");
        assert_eq!(percent(12.34), "12.3");
    }
}
