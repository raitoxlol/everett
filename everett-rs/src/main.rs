use clap::{Parser, Subcommand};

use everett::cli::{self, Args};

const SPAWNABLE: &[&str] = &["claude", "codex", "omp", "pi", "hermes", "grok", "devin"];
const ROUTERS: &[&str] = &["local", "jev"];
const EVENT_KINDS: &[&str] = &["done", "blocked", "needs-input", "info"];
const SCOPES: &[&str] = &["global", "project"];
const MODES: &[&str] = &["auto", "inbox", "resume"];
const LLMS: &[&str] = &["claude", "codex", "none"];
const HARNESSES: &[&str] = &["claude", "codex", "omp", "pi", "hermes", "grok", "devin"];

#[derive(Parser)]
#[command(name = "everett", version = env!("CARGO_PKG_VERSION"), about = "See and reach parallel coding-agent sessions.")]
struct Cli {
    /// look-back window (default 72)
    #[arg(long, global = true, default_value_t = 72.0)]
    hours: f64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// list recent sessions
    Ls {
        #[arg(long)]
        json: bool,
        /// include automated runs
        #[arg(long)]
        all: bool,
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(HARNESSES.to_vec()))]
        harness: Option<String>,
    },
    /// show card coverage across recent sessions
    Cards {
        /// rewrite existing auto cards with the current builder
        #[arg(long)]
        regenerate_auto: bool,
        /// with --regenerate-auto: report what would change, write nothing
        #[arg(long)]
        dry_run: bool,
        /// with --regenerate-auto: look-back window (default: the global --hours)
        #[arg(long)]
        hours: Option<f64>,
    },
    /// open a read-only local dashboard in your browser
    Dashboard {
        /// loopback port
        #[arg(long, default_value_t = 7347)]
        port: u16,
        /// print the URL without opening a browser
        #[arg(long)]
        no_open: bool,
    },
    /// pick the session a request belongs to (prints, never sends)
    Route {
        text: String,
        #[arg(long)]
        json: bool,
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(ROUTERS.to_vec()))]
        router: Option<String>,
    },
    /// deliver a request to the right session (or --to one) and print its reply
    Send {
        text: String,
        #[arg(long)]
        json: bool,
        /// route and print the command without resuming a session
        #[arg(long)]
        dry_run: bool,
        /// maximum wait for the session reply in seconds
        #[arg(long, default_value_t = 120.0)]
        timeout: f64,
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(ROUTERS.to_vec()))]
        router: Option<String>,
        /// skip routing: session id prefix, card name, or project folder
        #[arg(long, value_name = "SESSION")]
        to: Option<String>,
        /// when routing says NEW, start a new headless session
        #[arg(long)]
        spawn: bool,
        /// directory for --spawn
        #[arg(long)]
        dir: Option<String>,
        /// harness for --spawn (default: config default_harness, else claude)
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(SPAWNABLE.to_vec()))]
        harness: Option<String>,
        /// auto (default): inbox for a session with a live harness process, else headless resume
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(MODES.to_vec()), default_value = "auto")]
        mode: String,
        /// inbox mode: wait up to S seconds for the reply
        #[arg(long, value_name = "S", default_value_t = 0.0)]
        wait: f64,
    },
    /// record what this session is doing: done, blocked, needs-input, info
    Event {
        #[arg(value_parser = clap::builder::PossibleValuesParser::new(EVENT_KINDS.to_vec()))]
        kind: String,
        message: String,
        #[arg(long)]
        project: Option<String>,
        /// default: the calling session
        #[arg(long)]
        session: Option<String>,
    },
    /// recent events across sessions
    Events {
        /// window like 30m, 24h, 7d (default 24h)
        #[arg(long, default_value = "24h")]
        since: String,
        /// only this session (id prefix)
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        json: bool,
        /// also run the blocked-too-long escalation now
        #[arg(long)]
        check: bool,
    },
    /// get events from a session or project in your inbox
    Subscribe {
        /// session id prefix or name, project name, project:<name>, or *
        target: String,
        /// subscriber session (default: the calling session)
        #[arg(long = "as")]
        as_: Option<String>,
        /// unsubscribe
        #[arg(long)]
        remove: bool,
    },
    /// answer an Everett inbox message (goes to the sender's inbox)
    Reply { id: String, text: String },
    /// show (and mark delivered) the messages waiting for a session
    Inbox {
        /// default: the calling session, else the human inbox
        #[arg(long)]
        session: Option<String>,
        /// do not mark them delivered
        #[arg(long)]
        peek: bool,
        #[arg(long)]
        json: bool,
    },
    /// view: write the session list to your vault; merge: distill learnings into the core; schedule: nightly auto-merge via launchd
    Trunk {
        #[arg(value_parser = ["view", "merge", "schedule"], default_value = "view")]
        action: String,
        /// print instead of writing
        #[arg(long)]
        dry_run: bool,
        /// merge engine (default: config merge_llm, else claude)
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(LLMS.to_vec()))]
        llm: Option<String>,
        /// schedule: time of day HH:MM (default 04:00)
        #[arg(long, default_value = "04:00")]
        at: String,
        /// schedule: write the LaunchAgent and launchctl bootstrap it
        #[arg(long)]
        apply: bool,
        /// schedule: uninstall the LaunchAgent
        #[arg(long)]
        remove: bool,
    },
    /// push a fact to the shared core inbox (secrets are rejected)
    Learn {
        fact: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(SCOPES.to_vec()))]
        scope: Option<String>,
    },
    /// show the shared core, its file path, or merge history
    Core {
        #[arg(value_parser = ["show", "edit-path", "history"], default_value = "show")]
        action: String,
        #[arg(long)]
        project: Option<String>,
    },
    /// print (or --apply) the harness hook registrations
    InstallHooks {
        #[arg(long)]
        claude: bool,
        #[arg(long)]
        codex: bool,
        #[arg(long)]
        omp: bool,
        #[arg(long)]
        grok: bool,
        /// back up, then merge into the harness config
        #[arg(long)]
        apply: bool,
    },
    /// run the stdio MCP server (for harnesses; see install-mcp)
    Mcp,
    /// print (or --apply) the MCP server registration
    InstallMcp {
        #[arg(long)]
        claude: bool,
        #[arg(long)]
        codex: bool,
        #[arg(long)]
        omp: bool,
        #[arg(long)]
        grok: bool,
        /// back up, then register
        #[arg(long)]
        apply: bool,
        /// with --apply: refresh a stale or disabled Everett registration
        #[arg(long)]
        repair: bool,
    },
    /// check stores, hooks, cards, and router
    Doctor,
    /// friendly first-time setup (--yes for scripts)
    Onboard {
        /// non-interactive: apply the defaults without prompting
        #[arg(long)]
        yes: bool,
        /// backfill span in days, 1-30 (default 3)
        #[arg(long, default_value_t = 3)]
        span_days: i64,
        /// skip generating backfill cards
        #[arg(long)]
        no_backfill: bool,
        /// skip registering the MCP server
        #[arg(long)]
        no_mcp: bool,
        /// --yes mode: read a Jev key from this env var and save it
        #[arg(long)]
        jev_key_env: Option<String>,
        /// --yes mode: also schedule nightly `everett trunk merge` via launchd
        #[arg(long)]
        schedule_merge: bool,
    },
    /// internal: harness hook entry points (installed by `everett install-hooks`)
    #[command(hide = true)]
    Hook {
        stem: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    let mut a = Args {
        hours: cli.hours,
        ..Default::default()
    };
    let code = match cli.cmd {
        Cmd::Ls { json, all, harness } => {
            a.json = json;
            a.all = all;
            a.harness = harness;
            cli::cmd_ls(&a)
        }
        Cmd::Cards { regenerate_auto, dry_run, hours } => {
            a.regenerate_auto = regenerate_auto;
            a.dry_run = dry_run;
            a.regen_hours = hours;
            cli::cmd_cards(&a)
        }
        Cmd::Dashboard { port, no_open } => everett::dashboard::serve(a.hours, port, no_open),
        Cmd::Route { text, json, router } => {
            a.text = Some(text);
            a.json = json;
            a.router = router;
            cli::cmd_route(&a)
        }
        Cmd::Send { text, json, dry_run, timeout, router, to, spawn, dir, harness, mode, wait } => {
            a.text = Some(text);
            a.json = json;
            a.dry_run = dry_run;
            a.timeout = timeout;
            a.router = router;
            a.to = to;
            a.spawn = spawn;
            a.dir = dir;
            a.spawn_harness = harness;
            a.mode = mode;
            a.wait = wait;
            cli::cmd_send(&a)
        }
        Cmd::Event { kind, message, project, session } => {
            a.kind = Some(kind);
            a.message = Some(message);
            a.project = project;
            a.session = session;
            cli::cmd_event(&a)
        }
        Cmd::Events { since, session, json, check } => {
            a.since = since;
            a.session = session;
            a.json = json;
            a.check = check;
            cli::cmd_events(&a)
        }
        Cmd::Subscribe { target, as_, remove } => {
            a.target = Some(target);
            a.as_ = as_;
            a.remove = remove;
            cli::cmd_subscribe(&a)
        }
        Cmd::Reply { id, text } => {
            a.id = Some(id);
            a.text = Some(text);
            cli::cmd_reply(&a)
        }
        Cmd::Inbox { session, peek, json } => {
            a.session = session;
            a.peek = peek;
            a.json = json;
            cli::cmd_inbox(&a)
        }
        Cmd::Trunk { action, dry_run, llm, at, apply, remove } => {
            a.action = Some(action);
            a.dry_run = dry_run;
            a.llm = llm;
            a.at = at;
            a.apply = apply;
            a.remove = remove;
            cli::cmd_trunk(&a)
        }
        Cmd::Learn { fact, project, scope } => {
            a.fact = Some(fact);
            a.project = project;
            a.scope = scope;
            cli::cmd_learn(&a)
        }
        Cmd::Core { action, project } => {
            a.action = Some(action);
            a.project = project;
            cli::cmd_core(&a)
        }
        Cmd::InstallHooks { claude, codex, omp, grok, apply } => {
            a.claude = claude;
            a.codex = codex;
            a.omp = omp;
            a.grok = grok;
            a.apply = apply;
            cli::cmd_install_hooks(&a)
        }
        Cmd::Mcp => cli::cmd_mcp(&a),
        Cmd::InstallMcp { claude, codex, omp, grok, apply, repair } => {
            a.claude = claude;
            a.codex = codex;
            a.omp = omp;
            a.grok = grok;
            a.apply = apply;
            a.repair = repair;
            cli::cmd_install_mcp(&a)
        }
        Cmd::Doctor => cli::cmd_doctor(&a),
        Cmd::Onboard { yes, span_days, no_backfill, no_mcp, jev_key_env, schedule_merge } => {
            a.onboard_yes = yes;
            a.span_days = span_days;
            a.no_backfill = no_backfill;
            a.no_mcp = no_mcp;
            a.jev_key_env = jev_key_env;
            a.schedule_merge = schedule_merge;
            cli::cmd_onboard(&a)
        }
        Cmd::Hook { stem, args } => cli::cmd_hook(&stem, &args),
    };
    std::process::exit(code);
}
