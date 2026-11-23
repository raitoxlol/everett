//! Deterministic temporal expression parser for memory queries.
//! Port of teresis/temporal.py — same rules, same order, same windows.

use std::sync::OnceLock;

use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeZone, Utc};
use regex::Regex;

pub const DAY: f64 = 86400.0;

fn num_pat() -> String {
    format!(
        r"(?:[0-9]+|a\s+couple\s+(?:of\s+)?|a\s+few\s+|{})",
        NUM_WORDS_ORDER.join("|")
    )
}

fn month_pat() -> String {
    format!("(?:{})", MONTHS_ORDER.join("|"))
}

// Dict insertion order in Python — alternation order matters for leftmost-first match.
const NUM_WORDS_ORDER: &[&str] = &[
    "a", "an", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve", "couple", "few", "several",
];

const MONTHS_ORDER: &[&str] = &[
    "january", "jan", "february", "feb", "march", "mar", "april", "apr", "may", "june", "jun",
    "july", "jul", "august", "aug", "september", "sept", "sep", "october", "oct", "november",
    "nov", "december", "dec",
];

fn num_words(s: &str) -> Option<i64> {
    Some(match s {
        "a" | "an" | "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        "eleven" => 11,
        "twelve" => 12,
        "couple" => 2,
        "few" => 3,
        "several" => 4,
        _ => return None,
    })
}

fn month_num(name: &str) -> i64 {
    match name {
        "january" | "jan" => 1,
        "february" | "feb" => 2,
        "march" | "mar" => 3,
        "april" | "apr" => 4,
        "may" => 5,
        "june" | "jun" => 6,
        "july" | "jul" => 7,
        "august" | "aug" => 8,
        "september" | "sept" | "sep" => 9,
        "october" | "oct" => 10,
        "november" | "nov" => 11,
        "december" | "dec" => 12,
        _ => 0,
    }
}

macro_rules! re_once {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static RE: OnceLock<Regex> = OnceLock::new();
            RE.get_or_init(|| Regex::new(&$pat()).unwrap())
        }
    };
    ($name:ident, $pat:expr, literal) => {
        fn $name() -> &'static Regex {
            static RE: OnceLock<Regex> = OnceLock::new();
            RE.get_or_init(|| Regex::new($pat).unwrap())
        }
    };
}

fn ago_pat() -> String {
    format!(r"(?i)\b({})\s+(days?|weeks?|months?|years?)\s+ago\b", num_pat())
}
fn in_last_pat() -> String {
    format!(
        r"(?i)\b(?:in|over|during)\s+the\s+(?:last|past)\s+(?:({})\s+)?(days?|weeks?|months?|years?)\b",
        num_pat()
    )
}
fn month_day_pat() -> String {
    format!(
        r"(?i)\b(?:on\s+)?({})\s+([0-9]{{1,2}})(?:st|nd|rd|th)?(?:,?\s*(20[0-9]{{2}}))?\b",
        month_pat()
    )
}
fn day_month_pat() -> String {
    format!(
        r"(?i)\b([0-9]{{1,2}})(?:st|nd|rd|th)?\s+(?:of\s+)?({})(?:,?\s*(20[0-9]{{2}}))?\b",
        month_pat()
    )
}
fn in_month_pat() -> String {
    format!(r"(?i)\b(?:in|during|last|this)\s+({})(?:\s+(20[0-9]{{2}}))?\b", month_pat())
}
fn month_year_pat() -> String {
    format!(r"(?i)\b({})\s+(20[0-9]{{2}})\b", month_pat())
}

re_once!(re_ago, ago_pat);
re_once!(re_in_last, in_last_pat);
re_once!(re_last_unit, r"(?i)\blast\s+(week|month|year|weekend)\b", literal);
re_once!(re_yesterday, r"(?i)\b(yesterday|today)\b", literal);
re_once!(re_iso, r"\b(20[0-9]{2})-([0-9]{1,2})-([0-9]{1,2})\b", literal);
re_once!(re_month_day, month_day_pat);
re_once!(re_day_month, day_month_pat);
re_once!(re_in_month, in_month_pat);
re_once!(re_month_year, month_year_pat);
re_once!(re_in_year, r"\b(?:in|during)\s+(20[0-9]{2})\b", literal);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeWindow {
    pub start: f64,
    pub end: f64,
    /// Seconds outside [start, end] where proximity decays to 0.5
    pub soft_margin: f64,
}

impl TimeWindow {
    /// 1.0 inside [start, end], smoothly decaying outside.
    pub fn proximity(&self, ts: f64) -> f64 {
        if ts <= 0.0 {
            return 0.0;
        }
        if self.start <= ts && ts <= self.end {
            return 1.0;
        }
        let dist = if ts < self.start { self.start - ts } else { ts - self.end };
        1.0 / (1.0 + (dist / self.soft_margin.max(DAY)).powi(2))
    }
}

/// Python: strip, lower; isdigit -> int; "couple" in s -> 2; "few" in s -> 3; else map or 1.
fn parse_num(raw: &str) -> i64 {
    let s = raw.trim().to_lowercase();
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
        // Captures come from `[0-9]` patterns; clamp pathological sizes instead
        // of failing so a weird date never aborts the text search.
        return s.parse::<i64>().unwrap_or(i64::MAX);
    }
    if s.contains("couple") {
        return 2;
    }
    if s.contains("few") {
        return 3;
    }
    num_words(&s).unwrap_or(1)
}

fn utc(y: i32, m: u32, d: u32, end_of_day: bool) -> Option<f64> {
    let time = if end_of_day {
        NaiveTime::from_hms_opt(23, 59, 59)?
    } else {
        NaiveTime::from_hms_opt(0, 0, 0)?
    };
    let dt = NaiveDate::from_ymd_opt(y, m, d)?.and_time(time);
    Some(Utc.from_utc_datetime(&dt).timestamp() as f64)
}

fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let first_next = NaiveDate::from_ymd_opt(ny, nm, 1).unwrap();
    first_next.pred_opt().unwrap().day()
}

fn month_window(year: i32, month: u32) -> TimeWindow {
    let last_day = days_in_month(year, month);
    TimeWindow {
        start: utc(year, month, 1, false).unwrap(),
        end: utc(year, month, last_day, true).unwrap(),
        soft_margin: 10.0 * DAY,
    }
}

fn resolve_month_year(now_dt: &DateTime<Utc>, month: u32, explicit_year: Option<i32>) -> i32 {
    if let Some(y) = explicit_year {
        return y;
    }
    if month <= now_dt.month() {
        now_dt.year()
    } else {
        now_dt.year() - 1
    }
}

fn get_i64(m: &regex::Captures, idx: usize) -> Option<i64> {
    m.get(idx).and_then(|g| g.as_str().parse().ok())
}

/// Extract a target [start, end] unix timestamp window from `query` relative to `now`.
pub fn parse_time_window(query: &str, now: f64) -> Option<TimeWindow> {
    if query.is_empty() || now <= 0.0 {
        return None;
    }
    let now_dt = Utc.timestamp_opt(now as i64, 0).single()?;

    // 1. Exact ISO date: 2023-05-08
    if let Some(m) = re_iso().captures(query) {
        let y = get_i64(&m, 1).unwrap_or(0) as i32;
        let mo = get_i64(&m, 2).unwrap_or(0) as u32;
        let d = get_i64(&m, 3).unwrap_or(0) as u32;
        if (1..=12).contains(&mo) && (1..=31).contains(&d) {
            if let (Some(start), Some(end)) = (utc(y, mo, d, false), utc(y, mo, d, true)) {
                return Some(TimeWindow {
                    start: start - DAY,
                    end: end + DAY,
                    soft_margin: 5.0 * DAY,
                });
            }
        }
    }

    // 2. Month + day: "May 8", "on June 27, 2023", "8 May 2023"
    if let Some(m) = re_month_day().captures(query) {
        let mo = month_num(&m[1].to_lowercase()) as u32;
        let d = get_i64(&m, 2).unwrap_or(0) as u32;
        let y = resolve_month_year(&now_dt, mo, get_i64(&m, 3).map(|v| v as i32));
        if let (Some(start), Some(end)) = (utc(y, mo, d, false), utc(y, mo, d, true)) {
            return Some(TimeWindow {
                start: start - 2.0 * DAY,
                end: end + 2.0 * DAY,
                soft_margin: 7.0 * DAY,
            });
        }
    }
    if let Some(m) = re_day_month().captures(query) {
        let d = get_i64(&m, 1).unwrap_or(0) as u32;
        let mo = month_num(&m[2].to_lowercase()) as u32;
        let y = resolve_month_year(&now_dt, mo, get_i64(&m, 3).map(|v| v as i32));
        if let (Some(start), Some(end)) = (utc(y, mo, d, false), utc(y, mo, d, true)) {
            return Some(TimeWindow {
                start: start - 2.0 * DAY,
                end: end + 2.0 * DAY,
                soft_margin: 7.0 * DAY,
            });
        }
    }

    // 3. Relative "N days/weeks/months/years ago" (skip "how many days ago" questions!)
    if !query.to_lowercase().contains("how many") {
        if let Some(m) = re_ago().captures(query) {
            let n = parse_num(&m[1]) as f64;
            let unit = m[2].to_lowercase();
            let unit = unit.trim_end_matches('s');
            match unit {
                "day" => {
                    let center = now - n * DAY;
                    let half = 1.5_f64.max(0.35 * n) * DAY;
                    return Some(TimeWindow {
                        start: center - half,
                        end: center + half,
                        soft_margin: 3.0_f64.max(0.5 * n) * DAY,
                    });
                }
                "week" => {
                    let center = now - n * 7.0 * DAY;
                    let half = 4.0_f64.max(2.5 * n) * DAY;
                    return Some(TimeWindow {
                        start: center - half,
                        end: center + half,
                        soft_margin: 7.0_f64.max(4.0 * n) * DAY,
                    });
                }
                "month" => {
                    let center = now - n * 30.4 * DAY;
                    let half = 15.0_f64.max(8.0 * n) * DAY;
                    return Some(TimeWindow {
                        start: center - half,
                        end: center + half,
                        soft_margin: 20.0_f64.max(10.0 * n) * DAY,
                    });
                }
                "year" => {
                    let center = now - n * 365.25 * DAY;
                    let half = 60.0 * DAY;
                    return Some(TimeWindow {
                        start: center - half,
                        end: center + half,
                        soft_margin: 90.0 * DAY,
                    });
                }
                _ => {}
            }
        }
    }

    // 4. "yesterday" / "today"
    if let Some(m) = re_yesterday().captures(query) {
        let offset = if m[1].eq_ignore_ascii_case("yesterday") { 1.0 } else { 0.0 };
        let center = now - offset * DAY;
        return Some(TimeWindow {
            start: center - 1.5 * DAY,
            end: center + 1.5 * DAY,
            soft_margin: 3.0 * DAY,
        });
    }

    // 4b. "in the last month" / "over the past 2 weeks" -> rolling [now - duration, now]
    if let Some(m) = re_in_last().captures(query) {
        let n = m.get(1).map(|g| parse_num(g.as_str()) as f64).unwrap_or(1.0);
        let unit = m[2].to_lowercase();
        let unit = unit.trim_end_matches('s');
        let span = match unit {
            "day" => DAY,
            "week" => 7.0 * DAY,
            "month" => 31.0 * DAY,
            "year" => 366.0 * DAY,
            _ => DAY,
        } * n;
        return Some(TimeWindow {
            start: now - span,
            end: now,
            soft_margin: (3.0 * DAY).max(0.25 * span),
        });
    }

    // 5. "last week" / "last month" / "last year" / "last weekend"
    if let Some(m) = re_last_unit().captures(query) {
        let unit = m[1].to_lowercase();
        match unit.as_str() {
            "week" | "weekend" => {
                return Some(TimeWindow {
                    start: now - 11.0 * DAY,
                    end: now - 2.0 * DAY,
                    soft_margin: 7.0 * DAY,
                });
            }
            "month" => {
                let prev_m = if now_dt.month() == 1 { 12 } else { now_dt.month() - 1 };
                let prev_y = if now_dt.month() > 1 { now_dt.year() } else { now_dt.year() - 1 };
                return Some(month_window(prev_y, prev_m));
            }
            "year" => {
                return Some(TimeWindow {
                    start: utc(now_dt.year() - 1, 1, 1, false).unwrap(),
                    end: utc(now_dt.year() - 1, 12, 31, true).unwrap(),
                    soft_margin: 60.0 * DAY,
                });
            }
            _ => {}
        }
    }

    // 6. Explicit month ("in January", "February 2023")
    if let Some(m) = re_in_month().captures(query) {
        let mo = month_num(&m[1].to_lowercase()) as u32;
        let y = resolve_month_year(&now_dt, mo, get_i64(&m, 2).map(|v| v as i32));
        return Some(month_window(y, mo));
    }
    if let Some(m) = re_month_year().captures(query) {
        let mo = month_num(&m[1].to_lowercase()) as u32;
        let y = get_i64(&m, 2).unwrap_or(0) as i32;
        return Some(month_window(y, mo));
    }

    // 7. Explicit year ("in 2022")
    if let Some(m) = re_in_year().captures(query) {
        let y = get_i64(&m, 1).unwrap_or(0) as i32;
        return Some(TimeWindow {
            start: utc(y, 1, 1, false).unwrap(),
            end: utc(y, 12, 31, true).unwrap(),
            soft_margin: 60.0 * DAY,
        });
    }

    None
}
