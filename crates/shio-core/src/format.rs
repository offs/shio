pub fn format_speed(bytes_per_sec: u64) -> String {
    if bytes_per_sec == 0 {
        return "0 B/s".to_string();
    }
    format!("{}/s", bytesize::ByteSize(bytes_per_sec))
}

pub fn format_eta(downloaded: u64, total: Option<u64>, speed: u64) -> String {
    let remaining = match total {
        Some(t) if t > downloaded => t - downloaded,
        _ => return String::new(),
    };
    if speed == 0 {
        return String::new();
    }
    let secs = remaining / speed;
    match secs {
        0..=9 => "<10s".to_string(),
        10..=59 => format!("{}s", (secs / 5) * 5),
        60..=3599 => format!("{}m {:02}s", secs / 60, (secs % 60 / 10) * 10),
        _ => format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_eta_handles_empty_and_bucketed_values() {
        for (downloaded, total, speed, expected) in [
            (0, None, 1024, ""),
            (0, Some(1024), 0, ""),
            (0, Some(30), 1, "30s"),
            (0, Some(90), 1, "1m 30s"),
            (0, Some(7200), 1, "2h 00m"),
        ] {
            assert_eq!(format_eta(downloaded, total, speed), expected);
        }
    }
}
