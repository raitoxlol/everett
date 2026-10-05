use crate::session::{home, Session};

use super::omp::scan_root;

pub fn scan(since_hours: f64) -> Vec<Session> {
    scan_root(&home().join(".pi").join("agent").join("sessions"), "pi", since_hours)
}
