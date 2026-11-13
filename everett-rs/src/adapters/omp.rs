use crate::session::{home, Session};

use super::pi::scan_root;

pub fn scan(since_hours: f64) -> Vec<Session> {
    scan_root(&home().join(".omp").join("agent").join("sessions"), "omp", since_hours)
}
