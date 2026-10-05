use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};

/// `datetime.fromtimestamp(ts, utc).isoformat()` — microseconds only when non-zero.
pub fn iso_from_epoch(ts: f64) -> String {
    let secs = ts.floor() as i64;
    let micros = ((ts - secs as f64) * 1e6).round() as u32;
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
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d",
    ] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(ndt.and_utc().timestamp() as f64);
        }
    }
    None
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
