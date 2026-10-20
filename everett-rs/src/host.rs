//! Caller identity for harnesses whose MCP launch carries no session id (Pi, OMP, Devin,
//! Hermes). The binding comes only from the process tree, live records written by Everett's
//! own hooks, and the harness's session store -- never from tool arguments. When those do
//! not single out one session, the caller stays unbound.

use std::collections::HashMap;
use std::process::Command;
use std::sync::OnceLock;

use serde_json::{json, Map, Value};

use crate::session::now;

/// Harnesses `everett mcp --harness` can bind by finding its host process.
pub const DETECTABLE: &[&str] = &["pi", "omp", "devin", "hermes"];
const HARNESSES: &[&str] = &["claude", "codex", "omp", "pi", "hermes", "grok", "devin"];
const INTERPRETERS: &[&str] = &["node", "nodejs", "bun", "deno", "python", "python3", "sh", "bash", "zsh", "dash"];
/// Seconds of clock and `ps etime` rounding tolerated when comparing start times.
const SLACK: f64 = 2.0;
const MAX_DEPTH: usize = 16;

static LAUNCH: OnceLock<String> = OnceLock::new();

/// `everett mcp --harness <name>`: the host this server was registered for.
pub fn set_launch_harness(harness: &str) {
    let _ = LAUNCH.set(harness.to_string());
}

pub fn launch_harness() -> Option<&'static str> {
    LAUNCH.get().map(String::as_str).filter(|h| !h.is_empty())
}

#[derive(Clone, Debug)]
pub struct Host {
    pub harness: String,
    pub pid: i32,
    pub started: f64,
    pub cwd: String,
}

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub age: f64,
    pub args: String,
}

fn basename(token: &str) -> &str {
    token.rsplit('/').next().unwrap_or(token)
}

/// The harness a process command line runs, e.g. `pi`, `node /usr/local/bin/pi`,
/// `python3 ~/.local/bin/hermes chat`.
pub fn harness_of(args: &str) -> Option<&'static str> {
    let mut tokens = args.split_whitespace();
    let first = basename(tokens.next()?);
    let named = |name: &str| -> Option<&'static str> {
        let name = name.strip_suffix("-rpc").unwrap_or(name);
        HARNESSES.iter().find(|h| **h == name).copied()
    };
    if let Some(h) = named(first) {
        return Some(h);
    }
    let interpreter = first.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    if INTERPRETERS.contains(&first) || INTERPRETERS.contains(&interpreter) {
        for token in tokens {
            if token == "-m" {
                continue;
            }
            if token.starts_with("hermes_cli") {
                return Some("hermes");
            }
            if token.starts_with('-') {
                continue;
            }
            return named(basename(token));
        }
    }
    None
}

/// `[[dd-]hh:]mm:ss` from `ps -o etime`.
pub fn parse_etime(text: &str) -> Option<f64> {
    let text = text.trim();
    let (days, rest) = match text.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().ok()?, r),
        None => (0.0, text),
    };
    let mut seconds = 0.0;
    for part in rest.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86400.0 + seconds)
}

pub fn processes() -> HashMap<i32, Proc> {
    let mut out = HashMap::new();
    let Ok(output) = Command::new("ps").args(["-axww", "-o", "pid=,ppid=,etime=,args="]).output() else {
        return out;
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(etime)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let (Ok(pid), Ok(ppid), Some(age)) = (pid.parse::<i32>(), ppid.parse::<i32>(), parse_etime(etime)) else {
            continue;
        };
        out.insert(pid, Proc { pid, ppid, age, args: parts.collect::<Vec<_>>().join(" ") });
    }
    out
}

pub fn proc_cwd(pid: i32) -> Option<String> {
    if let Ok(path) = std::fs::read_link(format!("/proc/{}/cwd", pid)) {
        return Some(path.to_string_lossy().to_string());
    }
    let out = Command::new("lsof").args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix('n').map(str::to_string))
        .filter(|p| !p.is_empty())
}

fn same_dir(a: &str, b: &str) -> bool {
    let real = |p: &str| crate::paths::realpath(std::path::Path::new(p));
    !a.is_empty() && !b.is_empty() && real(a) == real(b)
}

/// The nearest harness process above `start`. With `want`, it must be that harness; without,
/// only DETECTABLE harnesses count (the others bind through their own env or request metadata).
pub fn find_host_in(
    procs: &HashMap<i32, Proc>,
    start: i32,
    want: Option<&str>,
) -> std::result::Result<Option<Host>, String> {
    let mut pid = start;
    for _ in 0..MAX_DEPTH {
        let Some(p) = procs.get(&pid) else { break };
        if let Some(h) = harness_of(&p.args) {
            if let Some(w) = want.filter(|w| *w != h) {
                return Err(format!(
                    "this server was registered with --harness {}, but the nearest harness process above it is {} (pid {})",
                    w, h, p.pid
                ));
            }
            if want.is_none() && !DETECTABLE.contains(&h) {
                return Ok(None);
            }
            let cwd = proc_cwd(p.pid)
                .or_else(|| std::env::current_dir().ok().map(|d| d.to_string_lossy().to_string()))
                .unwrap_or_default();
            return Ok(Some(Host { harness: h.to_string(), pid: p.pid, started: now() - p.age, cwd }));
        }
        if p.ppid <= 1 || p.ppid == pid {
            break;
        }
        pid = p.ppid;
    }
    match want {
        Some(w) => Err(format!("no {} process was found above this MCP server (pid {})", w, std::process::id())),
        None => Ok(None),
    }
}

fn find_host(want: Option<&str>) -> std::result::Result<Option<Host>, String> {
    static HOST: OnceLock<std::result::Result<Option<Host>, String>> = OnceLock::new();
    HOST.get_or_init(|| find_host_in(&processes(), std::os::unix::process::parent_id() as i32, want))
        .clone()
}

/// Newest live record an Everett hook wrote for this host process.
fn live_binding(host: &Host) -> Option<String> {
    let dir = crate::inbox::inbox_dir().join("live");
    let mut best: Option<(f64, String)> = None;
    for file in crate::paths::glob(&dir, "*.json") {
        let Some(sid) = file.file_stem().map(|s| s.to_string_lossy().to_string()) else { continue };
        if !crate::inbox::valid_id(&sid) {
            continue;
        }
        let Some(data) = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else {
            continue;
        };
        let pid = data.get("pid").and_then(Value::as_i64).unwrap_or(0) as i32;
        let ts = data.get("ts").and_then(Value::as_f64).unwrap_or(0.0);
        if pid == host.pid && ts >= host.started - SLACK && best.as_ref().is_none_or(|(t, _)| ts > *t) {
            best = Some((ts, sid));
        }
    }
    best.map(|(_, sid)| sid)
}

fn started_at(session: &crate::session::Session) -> Option<f64> {
    crate::timefmt::epoch_from_iso(&session.started)
}

/// The single session of `host.harness` created in the host's cwd since the host started.
/// A session or host without a cwd never matches, and two or more candidates is an error.
pub fn new_session_in_cwd<'a>(
    host: &Host,
    sessions: &'a [crate::session::Session],
) -> std::result::Result<&'a crate::session::Session, String> {
    if host.cwd.is_empty() {
        return Err(format!("the {} process (pid {}) has no known cwd; Everett will not guess", host.harness, host.pid));
    }
    let candidates: Vec<&crate::session::Session> = sessions
        .iter()
        .filter(|s| started_at(s).is_some_and(|t| t >= host.started - SLACK))
        .filter(|s| same_dir(&s.cwd, &host.cwd))
        .filter(|s| host.harness != "hermes" || crate::adapters::hermes::INTERACTIVE.contains(&s.source.as_str()))
        .filter(|s| crate::inbox::live(&s.id).is_none_or(|rec| rec.get("pid").and_then(Value::as_i64) == Some(host.pid as i64)))
        .collect();
    let session = match candidates.as_slice() {
        [one] => *one,
        [] => {
            return Err(format!(
                "no {} session created by this {} process (pid {}, cwd {}) was found yet",
                host.harness, host.harness, host.pid, host.cwd
            ))
        }
        many => {
            return Err(format!(
                "{} {} sessions were created in {} since this process started ({}); Everett cannot tell which one is calling",
                many.len(),
                host.harness,
                host.cwd,
                many.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", ")
            ))
        }
    };
    Ok(session)
}

/// [`new_session_in_cwd`], when no other live process of that harness could own it.
fn store_binding(host: &Host, procs: &HashMap<i32, Proc>) -> std::result::Result<String, String> {
    let hours = ((now() - host.started) / 3600.0 + 1.0).max(1.0);
    let sessions = crate::registry::scan(hours, false, None, &host.harness);
    let session = new_session_in_cwd(host, &sessions)?;
    let created = started_at(session).unwrap_or(host.started);
    let rivals: Vec<i32> = procs
        .values()
        .filter(|p| p.pid != host.pid && harness_of(&p.args) == Some(host.harness.as_str()))
        .filter(|p| now() - p.age <= created + SLACK)
        .filter(|p| proc_cwd(p.pid).is_some_and(|cwd| same_dir(&cwd, &host.cwd)))
        .map(|p| p.pid)
        .collect();
    if !rivals.is_empty() {
        return Err(format!(
            "{} other {} process(es) (pid {}) could own session {}; Everett will not guess",
            rivals.len(),
            host.harness,
            rivals.iter().map(i32::to_string).collect::<Vec<_>>().join(", "),
            session.id
        ));
    }
    Ok(session.id.clone())
}

pub fn bind(host: &Host, procs: &HashMap<i32, Proc>) -> std::result::Result<(String, String), String> {
    if let Some(sid) = live_binding(host) {
        return Ok((sid, "live record from its Everett hook".to_string()));
    }
    store_binding(host, procs).map(|sid| (sid, format!("only new {} session in {}", host.harness, host.cwd)))
}

/// Identity from the host process: Ok(None) when no detectable host is above this server.
pub fn identify(want: Option<&str>) -> std::result::Result<Option<Map<String, Value>>, String> {
    let Some(host) = find_host(want)? else { return Ok(None) };
    let (sid, how) = bind(&host, &processes()).map_err(|e| format!("{} (host {} pid {})", e, host.harness, host.pid))?;
    let mut out = Map::new();
    out.insert("session_id".into(), json!(sid));
    out.insert("harness".into(), json!(host.harness));
    out.insert("source".into(), json!(format!("host {} pid {}: {}", host.harness, host.pid, how)));
    Ok(Some(out))
}

/// One actionable sentence on how to bind a harness's MCP server to its session.
pub fn setup_hint(harness: &str) -> String {
    match harness {
        "pi" => "Run `everett install-hooks --pi --apply`; the Everett Pi extension registers the MCP server with each session's id. Then restart pi.".into(),
        "omp" => "Run `everett install-hooks --omp --apply` and `everett install-mcp --omp --repair --apply`, then restart omp.".into(),
        "devin" => "Run `everett install-hooks --devin --apply` and `everett install-mcp --devin --repair --apply`, then restart devin.".into(),
        "hermes" => "Register `everett mcp --harness hermes` in ~/.hermes/config.yaml (see `everett install-mcp --hermes`), restart hermes, and run one Hermes CLI session per directory; gateway mode cannot be bound.".into(),
        "claude" | "codex" | "grok" => format!("{} binds automatically; rerun `everett install-mcp --{} --repair --apply` and restart it.", harness, harness),
        _ => "Register Everett with `everett install-mcp --<harness> --apply` (it writes the right launcher), or set EVERETT_SESSION_ID in this MCP server's launch environment; see docs/mcp-identity.md.".into(),
    }
}
