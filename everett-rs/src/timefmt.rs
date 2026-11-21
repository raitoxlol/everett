use chrono::{DateTime, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// `datetime.fromtimestamp(ts, utc).isoformat()` — microseconds only when non-zero.
pub fn iso_from_epoch(ts: f64) -> String {
    let mut secs = ts.floor() as i64;
    let mut micros = ((ts - secs as f64) * 1e6).round() as u32;
    if micros == 1_000_000 {
        secs += 1;
        micros = 0;
    }
    let Some(dt) = Utc.timestamp_opt(secs, micros * 1000).single() else {
        return String::new();
    };
    if micros == 0 {
        dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        dt.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

/// `datetime.fromisoformat(v.replace('Z', '+00:00')).timestamp()` for the shapes stores emit.
pub fn epoch_from_iso(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9);
    }
    let naive = [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ]
    .iter()
    .find_map(|fmt| NaiveDateTime::parse_from_str(s, fmt).ok())
    .or_else(|| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0))?;
    // Python's datetime.fromisoformat().timestamp() reads naive stamps
    // as local time; match it (not UTC). An ambiguous fall-back stamp
    // takes the earlier instant, like Python's default fold=0.
    // chrono does not order the two candidates, so compare them.
    let ts = match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt.timestamp(),
        LocalResult::Ambiguous(a, b) => a.timestamp().min(b.timestamp()),
        LocalResult::None => naive.and_utc().timestamp(),
    };
    Some(ts as f64 + naive.and_utc().timestamp_subsec_nanos() as f64 / 1e9)
}

/// `_epoch` from devin.py: numbers >1e12 are milliseconds; strings parse as ISO or float.
pub fn epoch_from_any(v: &serde_json::Value) -> f64 {
    let x = match v {
        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
        serde_json::Value::String(s) => match epoch_from_iso(s) {
            Some(ts) => return ts,
            None => s.trim().parse::<f64>().unwrap_or(0.0),
        },
        _ => return 0.0,
    };
    if x > 1e12 { x / 1000.0 } else { x }
}

/// `time.strftime(fmt, time.localtime(ts))`.
pub fn strftime_local(fmt: &str, ts: f64) -> String {
    let secs = ts as i64;
    let dt = Local.timestamp_opt(secs, 0).single().unwrap_or_else(Local::now);
    dt.format(fmt).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naive_iso_uses_local_time_like_python() {
        // Python's datetime.fromisoformat().timestamp() reads naive stamps as
        // local time. In Tokyo (UTC+9) local midnight is UTC 15:00 the day before.
        // chrono caches the local zone, so assert in a re-exec'd test process.
        const STAMP: &str = "2025-01-01T00:00:00";
        if std::env::var_os("EVERETT_TZTEST").is_some() {
            let got = epoch_from_iso(STAMP).unwrap();
            assert_eq!(
                got,
                1735689600.0 - 9.0 * 3600.0,
                "naive stamp must follow local TZ"
            );
            assert_eq!(epoch_from_iso("2025-01-01T00:00:00Z").unwrap(), 1735689600.0);
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["timefmt::tests::naive_iso_uses_local_time_like_python", "--exact"])
            .env("EVERETT_TZTEST", "1")
            .env("TZ", "Asia/Tokyo")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("1 passed"), "child test did not run: {stdout}");
        assert_eq!(epoch_from_iso("2025-01-01T00:00:00Z").unwrap(), 1735689600.0);
    }

    #[test]
    fn naive_iso_keeps_fractional_seconds() {
        let whole = epoch_from_iso("2025-01-01T00:00:00").unwrap();
        let frac = epoch_from_iso("2025-01-01T00:00:00.250").unwrap();
        assert!((frac - whole - 0.25).abs() < 1e-6, "{frac} vs {whole}");
        let spaced = epoch_from_iso("2025-01-01 00:00:00.5").unwrap();
        assert!((spaced - whole - 0.5).abs() < 1e-6, "{spaced} vs {whole}");
    }

    #[test]
    fn ambiguous_fall_back_stamp_takes_earlier_instant() {
        // 2025-11-02 01:30 happens twice in New York; Python fold=0 picks EDT.
        if std::env::var_os("EVERETT_TZTEST_FOLD").is_some() {
            let got = epoch_from_iso("2025-11-02T01:30:00").unwrap();
            assert_eq!(got, epoch_from_iso("2025-11-02T01:30:00-04:00").unwrap());
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["timefmt::tests::ambiguous_fall_back_stamp_takes_earlier_instant", "--exact"])
            .env("EVERETT_TZTEST_FOLD", "1")
            .env("TZ", "America/New_York")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 passed"));
    }
}
