//! Display formatting for byte sizes and per-second rates.

/// Format a byte count as a short human-readable string:
/// `0 B`, `999 B`, `1.0K`, `1.5K`, `1.2M`, `45G`, `500G`, `2.0T`.
/// Values < 1024 use `B` with a leading space; larger values use the
/// largest fitting unit, with one decimal below 10 and no decimal at/above 10.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    if bytes < 1024 {
        return format!("{} B", bytes);
    }
    let mut value = bytes as f64;
    let mut idx = 0usize;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    let unit = UNITS[idx];
    if value >= 10.0 {
        format!("{:.0}{}", value, unit)
    } else {
        format!("{:.1}{}", value, unit)
    }
}

/// Format a per-second byte rate, e.g. `0 B/s`, `1.2M/s`.
/// Negative inputs (counter resets) clamp to zero.
pub fn human_rate(bytes_per_sec: f64) -> String {
    let b = if bytes_per_sec < 0.0 {
        0
    } else {
        bytes_per_sec.round() as u64
    };
    format!("{}/s", human_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_under_1k_use_b_with_space() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
    }

    #[test]
    fn bytes_scale_to_units() {
        assert_eq!(human_bytes(1024), "1.0K");
        assert_eq!(human_bytes(1536), "1.5K");
        assert_eq!(human_bytes(1_258_291), "1.2M");
        assert_eq!(human_bytes(48_318_382_080), "45G");
        assert_eq!(human_bytes(536_870_912_000), "500G");
    }

    #[test]
    fn rate_appends_per_second() {
        assert_eq!(human_rate(0.0), "0 B/s");
        assert_eq!(human_rate(1_258_291.0), "1.2M/s");
    }

    #[test]
    fn rate_clamps_negative_to_zero() {
        assert_eq!(human_rate(-5.0), "0 B/s");
    }
}
