use std::collections::BTreeMap;
use std::net::Ipv4Addr;
#[cfg(target_os = "macos")]
use std::process::Command;

use tiny_http::{Header, Method, Response, Server};

use crate::registry;
use crate::session::{now, Session};

const HARNESSES: &[(&str, &str)] = &[
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("omp", "OMP"),
    ("pi", "Pi"),
    ("hermes", "Hermes"),
    ("grok", "Grok CLI"),
    ("devin", "Devin CLI"),
    ("t3code", "T3 Code"),
];

pub fn serve(hours: f64, port: u16, no_open: bool) -> i32 {
    if !hours.is_finite() || hours <= 0.0 || port == 0 {
        eprintln!("dashboard needs a positive --hours value and a port from 1 to 65535.");
        return 2;
    }
    let address = (Ipv4Addr::LOCALHOST, port);
    let server = match Server::http(address) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("could not start the dashboard on http://127.0.0.1:{port}: {err}");
            return 1;
        }
    };
    let url = format!("http://127.0.0.1:{port}");
    println!("Everett dashboard: {url}");
    println!("Local only. Press Ctrl-C to stop.");
    if !no_open {
        open_browser(&url);
    }
    for request in server.incoming_requests() {
        let host = request
            .headers()
            .iter()
            .find(|header| header.field.equiv("Host"));
        let (status, content_type, body) =
            if !allowed_host(host.map(|header| header.value.as_str()), port) {
                (
                    403,
                    "text/plain; charset=utf-8",
                    "Use the dashboard's loopback URL.\n".to_string(),
                )
            } else if request.method() != &Method::Get {
                (
                    405,
                    "text/plain; charset=utf-8",
                    "This dashboard is read-only.\n".to_string(),
                )
            } else {
                match request.url().split('?').next().unwrap_or("/") {
                    "/" => {
                        let sessions = registry::scan(hours, false, Some(80), "");
                        (
                            200,
                            "text/html; charset=utf-8",
                            render(&sessions, hours, now()),
                        )
                    }
                    "/healthz" => (200, "text/plain; charset=utf-8", "ok\n".to_string()),
                    "/favicon.ico" => (204, "image/x-icon", String::new()),
                    _ => (404, "text/plain; charset=utf-8", "Not found\n".to_string()),
                }
            };
        let mut response = Response::from_string(body).with_status_code(status);
        for (name, value) in [
            ("Content-Type", content_type),
            ("Cache-Control", "no-store"),
            ("X-Content-Type-Options", "nosniff"),
            ("Referrer-Policy", "no-referrer"),
            ("Content-Security-Policy", "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'"),
        ] {
            response.add_header(Header::from_bytes(name, value).unwrap());
        }
        if let Err(err) = request.respond(response) {
            eprintln!("dashboard response failed: {err}");
        }
    }
    0
}

#[cfg(target_os = "macos")]
fn open_browser(url: &str) {
    if let Err(err) = Command::new("open").arg(url).status() {
        eprintln!("could not open a browser: {err}; open {url} manually.");
    }
}

#[cfg(not(target_os = "macos"))]
fn open_browser(_url: &str) {}

fn allowed_host(host: Option<&str>, port: u16) -> bool {
    matches!(host, Some(value) if value == format!("127.0.0.1:{port}") || value == format!("localhost:{port}"))
}

pub fn render(sessions: &[Session], hours: f64, at: f64) -> String {
    let active = sessions.iter().filter(|session| session.running).count();
    let attention = sessions
        .iter()
        .filter(|session| attention_state(session))
        .count();
    let cards = sessions
        .iter()
        .filter(|session| !session.card.is_empty())
        .count();
    let mut harness_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for session in sessions {
        *harness_counts.entry(harness_key(session)).or_default() += 1;
    }
    let rows = if sessions.is_empty() {
        empty_state()
    } else {
        sessions
            .iter()
            .enumerate()
            .map(|(index, session)| session_row(session, index, at))
            .collect()
    };
    let coverage = HARNESSES
        .iter()
        .map(|(key, label)| {
            let count = harness_counts.get(key).copied().unwrap_or(0);
            format!(
                "<li><span class=\"coverage-mark{}\" aria-hidden=\"true\"></span><span>{}</span><strong>{}</strong></li>",
                if count > 0 { " detected" } else { "" },
                html(label),
                count
            )
        })
        .collect::<String>();
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="light">
<title>Everett · Recent sessions</title>
<style>{CSS}</style>
</head>
<body>
<a class="skip" href="#sessions">Skip to sessions</a>
<div class="app-shell">
  <aside class="rail" aria-label="Everett">
    <a class="brand" href="#top" aria-label="Everett dashboard"><span class="brand-mark">E</span><span>Everett</span></a>
    <nav aria-label="Dashboard sections">
      <a class="active" href="#sessions" aria-current="location"><span>Sessions</span></a>
      <a href="#activity"><span>Activity</span></a>
      <a href="#core"><span>Shared core</span></a>
      <a href="#setup"><span>Setup</span></a>
    </nav>
    <div class="local-note"><span></span><strong>Local</strong><small>127.0.0.1</small></div>
  </aside>
  <main id="top">
    <header class="page-head">
      <div><h1>Recent sessions</h1><p>One local view of every agent Everett can see.</p></div>
      <button class="refresh" type="button" onclick="location.reload()">Refresh</button>
    </header>
    <section class="dispatch-summary" aria-label="Session summary">
      <div><strong>{}</strong><span>sessions in the last {}</span></div>
      <div><strong>{active}</strong><span>appear active</span></div>
      <div class="attention"><strong>{attention}</strong><span>need attention</span></div>
      <div><strong>{cards}</strong><span>with Everett cards</span></div>
    </section>
    <section class="workspace" id="sessions">
      <div class="workspace-head">
        <div><h2>Session manifest</h2><p id="visible-count" aria-live="polite">{} shown</p></div>
        <div class="filters" role="search">
          <label class="search"><span class="sr-only">Search sessions</span><input id="search" type="search" placeholder="Search project, card, or ID" autocomplete="off"></label>
          <label><span class="sr-only">Filter by harness</span><select id="harness-filter"><option value="">All harnesses</option>{}</select></label>
          <label><span class="sr-only">Filter by state</span><select id="state-filter"><option value="">Any state</option><option value="running">Active</option><option value="attention">Needs attention</option><option value="quiet">Quiet</option></select></label>
        </div>
      </div>
      <div class="manifest-head" aria-hidden="true"><span>Activity</span><span>Session</span><span>Updated</span></div>
      <div class="manifest" id="activity">{rows}</div>
      <div class="filtered-empty" id="filtered-empty" hidden><h3>No sessions match</h3><p>Clear the filters to return to the full manifest.</p><button type="button" id="reset-filters">Reset filters</button></div>
    </section>
    <section class="lower-grid">
      <article class="coverage" aria-labelledby="coverage-title"><div><h2 id="coverage-title">Harness coverage</h2><p>Detected in this {}</p></div><ul>{coverage}</ul></article>
      <article class="runbook" id="core"><h2>Shared core</h2><p>Project knowledge shared through Everett's hooks and MCP.</p><code>everett core</code><button type="button" data-copy="everett core">Copy command</button></article>
      <article class="runbook" id="setup"><h2>Setup check</h2><p>Inspect stores, hooks, cards, and router health from the CLI.</p><code>everett doctor</code><button type="button" data-copy="everett doctor">Copy command</button></article>
    </section>
    <footer><span>Read-only local dashboard</span><span>Provider stores remain untouched</span></footer>
  </main>
</div>
<script>{JS}</script>
</body>
</html>"##,
        sessions.len(),
        hours_label(hours),
        sessions.len(),
        harness_options(),
        hours_label(hours),
    )
}

fn session_row(session: &Session, index: usize, at: f64) -> String {
    let state = state_class(session);
    let project = project_name(&session.cwd);
    let label = harness_label(session);
    let summary = if !session.card.is_empty() {
        session.card.as_str()
    } else if !session.title.is_empty() {
        session.title.as_str()
    } else if !session.first_user.is_empty() {
        session.first_user.as_str()
    } else {
        "No Everett card yet. Run the session once with hooks installed to create one."
    };
    let card_source = session.card_source.as_deref().unwrap_or("none");
    let status = status_label(session);
    let command = format!(
        "everett send \"Your message\" --to {} --mode inbox",
        crate::proc::shlex_quote(&session.id)
    );
    let search = format!(
        "{} {} {} {} {}",
        label, project, summary, session.id, status
    )
    .to_lowercase();
    format!(
        r#"<article class="session-row {state}" data-session data-search="{}" data-harness="{}" data-state="{}">
  <div class="route-stop" aria-hidden="true"><span>{}</span></div>
  <details>
    <summary>
      <span class="session-main"><span class="harness">{}</span><strong>{}</strong><span class="summary-text">{}</span></span>
      <span class="session-meta"><span class="status">{}</span><time>{}</time><span class="chevron" aria-hidden="true"></span></span>
    </summary>
    <div class="session-detail">
      <div class="card-copy"><span>Everett card · {}</span><p>{}</p></div>
      <dl><div><dt>Harness</dt><dd>{}</dd></div><div><dt>Session ID</dt><dd>{}</dd></div><div><dt>Working directory</dt><dd>{}</dd></div><div><dt>Source</dt><dd>{}</dd></div><div><dt>Latest event</dt><dd>{}</dd></div></dl>
      <div class="command"><code>{}</code><button type="button" data-copy="{}">Copy send command</button></div>
      <p class="command-note">Run in your terminal to queue an inbox message. Pickup depends on hooks or polling.</p>
    </div>
  </details>
</article>"#,
        attr(&search),
        attr(harness_key(session)),
        attr(filter_state(session)),
        index + 1,
        html(label),
        html(&project),
        html(summary),
        html(status),
        html(&ago(session.last_active, at)),
        html(card_source),
        html(summary),
        html(label),
        html(&session.id),
        html(&session.cwd),
        html(if session.source.is_empty() {
            "native store"
        } else {
            &session.source
        }),
        html(if session.state.is_empty() {
            "No event recorded"
        } else {
            &session.state
        }),
        html(&command),
        attr(&command),
    )
}

fn empty_state() -> String {
    r#"<div class="empty-state"><span class="empty-mark" aria-hidden="true"></span><div><h3>No recent sessions yet</h3><p>Start a supported coding agent, then refresh. If one should be here, run <code>everett doctor</code>.</p></div></div>"#.to_string()
}

fn harness_options() -> String {
    HARNESSES
        .iter()
        .map(|(key, label)| format!("<option value=\"{}\">{}</option>", attr(key), html(label)))
        .collect()
}

fn harness_key(session: &Session) -> &str {
    if session.source == "t3code" {
        "t3code"
    } else {
        session.harness.as_str()
    }
}

fn harness_label(session: &Session) -> &str {
    HARNESSES
        .iter()
        .find(|(key, _)| *key == harness_key(session))
        .map(|(_, label)| *label)
        .unwrap_or(&session.harness)
}

fn state_class(session: &Session) -> &'static str {
    if attention_state(session) {
        "is-attention"
    } else if session.running {
        "is-running"
    } else {
        "is-quiet"
    }
}

fn filter_state(session: &Session) -> &'static str {
    if attention_state(session) {
        "attention"
    } else if session.running {
        "running"
    } else {
        "quiet"
    }
}

fn status_label(session: &Session) -> &str {
    match session.state_kind.as_str() {
        "blocked" => "Blocked",
        "needs-input" => "Needs input",
        "done" => "Done",
        "info" => "Updated",
        _ if session.running => "Active",
        _ => "Quiet",
    }
}

fn attention_state(session: &Session) -> bool {
    matches!(session.state_kind.as_str(), "blocked" | "needs-input")
}

fn project_name(cwd: &str) -> String {
    cwd.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or("Unknown project")
        .to_string()
}

fn ago(ts: f64, at: f64) -> String {
    let seconds = (at - ts).max(0.0) as u64;
    match seconds {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

fn hours_label(hours: f64) -> String {
    if (hours % 24.0).abs() < f64::EPSILON {
        let days = hours / 24.0;
        format!("{days} {}", if days == 1.0 { "day" } else { "days" })
    } else {
        format!("{hours} {}", if hours == 1.0 { "hour" } else { "hours" })
    }
}

fn html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn attr(value: &str) -> String {
    html(value).replace(['\n', '\r'], " ")
}

const CSS: &str = include_str!("dashboard.css");
const JS: &str = include_str!("dashboard.js");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_dashboard_escapes_provider_content() {
        let mut session = Session::new("claude", "abc<123", "/tmp/a&b", "", "", 90.0);
        session.card = "Fix <script>alert('x')</script>".into();
        session.running = true;
        let page = render(&[session], 72.0, 100.0);
        assert!(page.contains("Fix &lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"));
        assert!(!page.contains("<script>alert"));
        assert!(page.contains("a&amp;b"));
    }

    #[test]
    fn dashboard_preserves_honest_attention_state() {
        let mut session = Session::new("codex", "codex-1", "/tmp/everett", "", "", 0.0);
        session.state_kind = "needs-input".into();
        session.card = "Waiting for the owner decision".into();
        let page = render(&[session], 72.0, 7_200.0);
        assert!(page.contains("Needs input"));
        assert!(page.contains("data-state=\"attention\""));
        assert!(page.contains("2h ago"));
    }

    #[test]
    fn empty_dashboard_teaches_recovery() {
        let page = render(&[], 72.0, 0.0);
        assert!(page.contains("No recent sessions yet"));
        assert!(page.contains("everett doctor"));
    }

    #[test]
    fn local_host_boundary_rejects_rebinding_hosts() {
        assert!(allowed_host(Some("127.0.0.1:7347"), 7347));
        assert!(allowed_host(Some("localhost:7347"), 7347));
        assert!(!allowed_host(Some("attacker.example:7347"), 7347));
        assert!(!allowed_host(None, 7347));
    }

    #[test]
    fn all_supported_harnesses_and_t3_source_are_visible() {
        let mut session = Session::new("codex", "t3-thread", "/work/app", "", "", 0.0);
        session.source = "t3code".to_string();
        let page = render(&[session], 72.0, 900.0);
        for (_, label) in HARNESSES {
            assert!(page.contains(label));
        }
        assert!(page.contains("data-harness=\"t3code\""));
    }
}
