pub mod claude;
pub mod codex;
pub mod devin;
pub mod external;
pub mod grok;
pub mod hermes;
pub mod omp;
pub mod pi;
pub(crate) mod t3_cli;
pub mod t3code;

pub(crate) fn scan(name: &str, since_hours: f64) -> Vec<crate::session::Session> {
    use crate::harness::Native;
    match Native::parse(name) {
        Some(Native::Claude) => claude::scan(since_hours),
        Some(Native::Codex) => codex::scan(since_hours),
        Some(Native::Omp) => omp::scan(since_hours),
        Some(Native::Pi) => pi::scan(since_hours),
        Some(Native::Hermes) => hermes::scan(since_hours),
        Some(Native::Grok) => grok::scan(since_hours),
        Some(Native::Devin) => devin::scan(since_hours),
        None => Vec::new(),
    }
}
