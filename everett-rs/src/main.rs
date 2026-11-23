use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use everett::cli::{self, Args};

const SPAWNABLE: &[&str] = everett::harness::SPAWNABLE;
const ROUTERS: &[&str] = &["local", "jev"];
const EVENT_KINDS: &[&str] = &["done", "blocked", "needs-input", "info"];
const SCOPES: &[&str] = &["global", "project"];
const MODES: &[&str] = everett::send::MODES;
const LLMS: &[&str] = &["claude", "codex", "none"];
const HARNESSES: &[&str] = everett::harness::ALL;

/// Top-level help lists commands by what a newcomer needs first, not by declaration order.
const HELP_GROUPS: &[(&str, &[&str])] = &[
    ("Start here", &["onboard", "dashboard", "ls", "doctor"]),
    (
        "Work across sessions",
        &["send", "route", "inbox", "reply", "sent", "status", "retry", "proof", "event", "events", "subscribe"],
    ),
    ("Cross-org shared threads", &["thread"]),
    ("Shared memory", &["learn", "recall", "core", "trunk", "cards", "skills"]),
    (
        "Setup and account",
        &["install-hooks", "install-mcp", "mcp", "login", "whoami", "identity", "logout"],
    ),
    (
        "Advanced: devices and external agents",
        &["relay", "peer", "devices", "gateway", "gateway-permissions", "external", "tunnel"],
    ),
];

fn command() -> clap::Command {
    let cmd = Cli::command();
    let mut groups = String::new();
    for (heading, names) in HELP_GROUPS {
        groups.push_str(&format!("{heading}:\n"));
        for name in *names {
            let about = cmd
                .find_subcommand(name)
                .and_then(|s| s.get_about())
                .map(|a| a.to_string())
                .unwrap_or_default();
            groups.push_str(&format!("  {name:<14} {about}\n"));
        }
        groups.push('\n');
    }
    cmd.help_template(format!(
        "{{about-with-newline}}\n{{usage-heading}} {{usage}}\n\n{groups}Options:\n{{options}}\n\nRun `everett <command> --help` for details.\n"
    ))
}

#[derive(Parser)]
#[command(name = "everett", version = env!("CARGO_PKG_VERSION"), long_version = concat!(env!("CARGO_PKG_VERSION"), "\nrevision: ", env!("EVERETT_BUILD_REVISION"), "\ndirty: ", env!("EVERETT_BUILD_DIRTY")), about = "See and reach parallel coding-agent sessions.")]
struct Cli {
    /// look-back window (default 72)
    #[arg(long, global = true, default_value_t = 72.0)]
    hours: f64,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(hide = true)]
    GatewayWorker,
    /// check an explicitly selected local wake path end to end
    Proof {
        #[command(subcommand)]
        action: ProofCmd,
    },
    /// list this account's registered device metadata, or manage one device
    Devices {
        #[command(subcommand)]
        action: Option<Devices>,
        #[arg(long)]
        json: bool,
    },
    /// encrypted Cloudflare receiver; inbox delivery with optional owner wake policies
    Relay {
        #[command(subcommand)]
        action: Option<RelayCmd>,
    },
    /// CLI-first encrypted shared text threads between two inboxes
    Thread {
        #[command(subcommand)]
        action: ThreadCmd,
    },
    /// pair devices and make restricted SSH inbox requests
    Peer {
        #[command(subcommand)]
        action: PeerCmd,
    },
    /// keep an OpenAI tunnel-client forwarding
    Tunnel {
        #[command(subcommand)]
        action: TunnelCmd,
    },
    /// serve restricted owner-bound MCP for external agents
    Gateway {
        #[arg(long)]
        config: std::path::PathBuf,
        #[arg(long, default_value = "stdio", value_parser = ["stdio", "http"])]
        transport: String,
        #[arg(long, default_value_t = 8788)]
        port: u16,
    },
    /// inspect or edit local cross-org access and permission settings
    #[command(alias = "permissions")]
    Access {
        /// custom access policy path (defaults to ~/.everett/access.json)
        #[arg(long, global = true)]
        config: Option<std::path::PathBuf>,
        #[arg(long, global = true)]
        json: bool,
        #[command(subcommand)]
        action: Option<AccessCmd>,
    },
    /// inspect or edit the tool grant for an owner MCP connection
    GatewayPermissions {
        #[arg(long)]
        file: std::path::PathBuf,
        #[arg(long, conflicts_with_all = ["allow", "deny_all"])]
        all: bool,
        #[arg(long, value_delimiter = ',', conflicts_with = "deny_all")]
        allow: Vec<String>,
        #[arg(long)]
        deny_all: bool,
    },
    /// register and manage owner-authorized external inboxes
    External {
        #[command(subcommand)]
        action: ExternalCmd,
    },
    /// Inspect owner-local T3 Code threads
    Control {
        #[command(subcommand)]
        action: ControlCmd,
    },
    /// list recent sessions
    Ls {
        /// list only this paired device (local listing is unchanged by default)
        #[arg(long)]
        peer: Option<String>,
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
    /// list or print skills from every harness's skill directories (the MCP skill hub)
    Skills {
        /// print this skill's instructions instead of listing
        #[arg(long)]
        get: Option<String>,
        /// only skills from this source (claude, codex, agents, devin, hermes, claude-plugins, devin-plugins, project, extra)
        #[arg(long)]
        source: Option<String>,
        /// print the one skill that fits this task, or nothing
        #[arg(long = "match", value_name = "TASK")]
        task: Option<String>,
        /// list the few skills named up front in this project
        #[arg(long)]
        shortlist: bool,
        /// filter the list by name or description
        query: Option<String>,
        #[arg(long)]
        json: bool,
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
        /// retry a remote inbox send without posting it twice
        #[arg(long)]
        idempotency_key: Option<String>,
        #[arg(long)]
        json: bool,
        /// preview the transport or resume command without sending or queueing
        #[arg(long)]
        dry_run: bool,
        /// maximum wait for the session reply in seconds
        #[arg(long, default_value_t = 120.0)]
        timeout: f64,
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(ROUTERS.to_vec()))]
        router: Option<String>,
        /// skip routing: session ID prefix, exact T3 thread ID, card name, or project folder
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
        /// ping (default): immediate delivery or an explicit error; inbox is legacy queue-only delivery
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(MODES.to_vec()), default_value = "ping")]
        mode: String,
        /// inbox mode: wait up to S seconds for the reply
        #[arg(long, value_name = "S", default_value_t = 0.0)]
        wait: f64,
        /// also wake one exact local Claude background session after queueing
        #[arg(long)]
        wake: bool,
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
    /// list up to 50 recent local requests you sent
    Sent {
        /// list requests sent to this exact recipient session ID
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// show pickup and any reply for a local request you sent
    Status {
        message_id: String,
        #[arg(long)]
        json: bool,
    },
    /// nudge T3 again for a queued request you sent
    Retry {
        message_id: String,
        #[arg(long)]
        json: bool,
    },
    /// show (and mark delivered) the messages waiting for a session
    Inbox {
        /// default: the calling session, else the human inbox
        #[arg(long)]
        session: Option<String>,
        /// do not mark them delivered
        #[arg(long)]
        peek: bool,
        /// only show and mark delivered this message id
        #[arg(long)]
        message: Option<String>,
        /// only show replies to this original message id
        #[arg(long)]
        reply_to: Option<String>,
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
        /// schedule: write and activate a launchd job or systemd user timer
        #[arg(long)]
        apply: bool,
        /// schedule: uninstall the native user schedule
        #[arg(long)]
        remove: bool,
    },
    /// push a fact to the shared core inbox (secrets are rejected)
    Learn {
        fact: String,
        /// Keep the fact searchable locally for this many seconds (1 to 31536000).
        #[arg(long)]
        ttl_seconds: Option<u64>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(SCOPES.to_vec()))]
        scope: Option<String>,
        /// Stable name for a fact that changes, such as user.editor; a new value replaces the old one in memory search
        #[arg(long)]
        key: Option<String>,
    },
    /// search every learned fact in the shared memory.db (word, stem, and typo-tolerant)
    Recall {
        query: String,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(SCOPES.to_vec()))]
        scope: Option<String>,
        /// Maximum results (1 to 50)
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=50), default_value_t = 5)]
        limit: u8,
        /// Print the result object on one line
        #[arg(long)]
        json: bool,
        /// Show each memory's score and ranking detail
        #[arg(long)]
        explain: bool,
    },
    /// show or search local core memory, locate its file, or list merge history
    Core {
        #[command(subcommand)]
        action: Option<CoreCmd>,
        #[arg(long, global = true)]
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
        /// Pi extension: exposes Everett tools through Pi's registerTool API per session
        #[arg(long)]
        pi: bool,
        /// Devin CLI hooks that bind `everett mcp --harness devin` to the session
        #[arg(long)]
        devin: bool,
        /// back up, then merge into the harness config
        #[arg(long)]
        apply: bool,
    },
    /// run the stdio MCP server (for harnesses; see install-mcp)
    Mcp {
        /// bind callers to sessions of this host harness (pi, omp, devin, hermes)
        #[arg(long, value_parser = ["pi", "omp", "devin", "hermes"])]
        harness: Option<String>,
    },
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
        #[arg(long)]
        devin: bool,
        /// via the Pi extension (same as install-hooks --pi)
        #[arg(long)]
        pi: bool,
        /// print the ~/.hermes/config.yaml entry (not applied automatically)
        #[arg(long)]
        hermes: bool,
        /// back up, then register
        #[arg(long)]
        apply: bool,
        /// with --apply: refresh a stale or disabled Everett registration
        #[arg(long)]
        repair: bool,
    },
    /// sign in to your Everett account (Cloudflare device approval)
    Login {
        /// Cloudflare enrollment (config: auth.provider)
        #[arg(long, value_parser = ["cloudflare"])]
        provider: Option<String>,
        /// Cloudflare identity issuer (otherwise auth.issuer or https://everett.raitox.tech)
        #[arg(long)]
        issuer: Option<String>,
        /// Cloudflare relay URL (otherwise auth.base_url, auth.issuer, or https://relay.raitox.tech)
        #[arg(long)]
        base_url: Option<String>,
        /// cancel the pending Cloudflare approval request
        #[arg(long, conflicts_with_all = ["force", "open", "no_wait", "issuer", "base_url", "device"])]
        cancel: bool,
        /// start or show a pending Cloudflare approval without waiting
        #[arg(long)]
        no_wait: bool,
        /// name this device in the account (default: hostname)
        #[arg(long)]
        device: Option<String>,
        /// open the verification URL in a browser (never automatic)
        #[arg(long, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", default_value = "false", require_equals = true)]
        open: bool,
        /// start a fresh sign-in even when already signed in
        #[arg(long)]
        force: bool,
        /// machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// diagnose identity mapping with read-only account verification
    Identity {
        #[command(subcommand)]
        action: IdentityCmd,
    },
    /// print the signed-in account
    Whoami {
        #[arg(long, value_parser = ["cloudflare"])]
        provider: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// revoke this device's sign-in and delete the local token
    Logout {
        #[arg(long, value_parser = ["cloudflare"])]
        provider: Option<String>,
        /// revoke this Cloudflare device before signing out
        #[arg(long)]
        revoke_device: bool,
        #[arg(long)]
        json: bool,
    },
    /// check stores, hooks, cards, and router
    Doctor,
    /// friendly first-time setup; handoff choices require terminal confirmation
    Onboard {
        /// apply validated choices from a website setup guide
        #[arg(long, value_name = "TOKEN", allow_hyphen_values = true)]
        handoff: Option<String>,
        /// inspect a handoff without writing files (requires --handoff)
        #[arg(long)]
        handoff_preview: bool,
        /// non-interactive: apply the defaults without prompting
        #[arg(long)]
        yes: bool,
        /// backfill span in days, 1-30 (default 3)
        #[arg(long)]
        span_days: Option<i64>,
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

#[derive(Subcommand)]
enum Devices {
    /// permanently revoke a device and remove its local peer bindings
    Revoke { device_id: String },
}

#[derive(Subcommand)]
enum ControlCmd {
    /// Read recent T3 thread or local session messages as JSON
    Read {
        #[arg(required_unless_present = "session", conflicts_with = "session")]
        thread: Option<String>,
        /// Local Everett session ID or unambiguous prefix
        #[arg(long)]
        session: Option<String>,
        #[arg(long, default_value_t = 10)]
        messages: usize,
    },
    /// List T3 threads as JSON, optionally filtered by project
    Threads {
        #[arg(long)]
        project: Option<String>,
    },
}

#[derive(Subcommand)]
enum CoreCmd {
    Show,
    EditPath,
    History,
    /// Search learned facts from global memory and the selected project
    Search {
        query: String,
        /// Print structured results with fact metadata
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum PeerCmd {
    /// pin a device on the selected relay after fingerprint confirmation
    AddRelay {
        name: String,
        #[arg(long)]
        device: String,
        #[arg(long)]
        fingerprint: String,
    },
    /// pair one SSH direction; replies return through status polling
    Add {
        name: String,
        #[arg(long)]
        ssh: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        /// absolute path to Everett on the remote machine
        #[arg(long)]
        binary: String,
        /// remote Ed25519 host-key SHA256 fingerprint, confirmed out of band
        #[arg(long)]
        host_key: String,
        /// remote Everett device-key fingerprint, confirmed out of band
        #[arg(long)]
        device_key: String,
    },
    /// show paired devices
    List,
    /// print this device's peer id and public key
    Identity,
    /// remove a peer; duplicate transport aliases require --transport ssh|cloudflare
    Remove {
        name: String,
        /// select one transport when a legacy alias exists in both registries
        #[arg(long, value_parser = ["ssh", "cloudflare"])]
        transport: Option<String>,
    },
    /// remove a client's tagged key from this machine's authorized_keys
    Revoke { client: String },
    /// list sessions on a peer
    Ls {
        name: String,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        harness: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// fetch one remote session card
    Card { target: String },
    /// poll observed delivery state and import any reply into the original sender's inbox
    Status { name: String, message_id: String },
    /// serve one bounded stdin/stdout RPC (invoked by sshd's forced command)
    Serve {
        #[arg(long)]
        client: Option<String>,
    },
    #[command(hide = true)]
    Install,
}

fn peer_output(result: everett::error::Result<serde_json::Value>) -> i32 {
    match result {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value).unwrap());
            0
        }
        Err(e) => {
            eprintln!("everett: {e}");
            e.code
        }
    }
}

#[derive(Subcommand)]
enum RelayCmd {
    /// read-only offline receiver, registration, webhook and bounded cache diagnostics as JSON
    Diagnose,
    /// list exposed, authenticated author/session cards in this account (no pairing)
    Authors,
    /// expose one exact local session and its card to authenticated account peers
    Expose {
        session: String,
        #[arg(long)]
        harness: Option<String>,
        #[arg(long)]
        remove: bool,
    },
    /// register this device and print its public bundle and fingerprint
    Register,
    /// list signed bundles; use --names for advisory hosted device labels
    #[command(aliases = ["credentials", "directory"])]
    Devices {
        /// readable labels with IDs/fingerprints; never changes pairing selectors
        #[arg(long, conflicts_with = "json")]
        names: bool,
        /// original JSON bundle array (also the default)
        #[arg(long)]
        json: bool,
    },
    /// rename this Cloudflare device's advisory hosted label, not its ID or pins
    Rename { name: String },
    /// configure a same-owner webhook wake for one registered local inbox
    Webhook {
        session: String,
        #[arg(long)]
        harness: String,
        #[arg(long)]
        url_file: std::path::PathBuf,
        #[arg(long)]
        bearer_file: std::path::PathBuf,
        #[arg(long, default_value_t = 30)]
        cooldown_seconds: u64,
    },
    /// let all verified same-account senders wake an already configured external inbox
    TrustAccount {
        session: String,
        #[arg(long)]
        harness: String,
        /// preview the exact change without writing policy, backup or ledger files
        #[arg(long)]
        dry_run: bool,
    },
    /// verified namespace, agent credential ID and authoritative role
    Whoami,
    /// permanently revoke this or an admin-selected agent/device credential
    #[command(alias = "revoke-device")]
    RevokeAgent { device: String },
    /// permanently revoke an org membership (admin only)
    RevokeMember { subject: String },
    /// rotate both independent relay keys; requires the old signing key
    Rotate,
    /// claim a LOCAL@SCOPE named inbox bound to an exact harness/session (or --remove it)
    RegisterInbox {
        address: String,
        #[arg(long)]
        harness: String,
        #[arg(long)]
        session: String,
        #[arg(long)]
        device: Option<String>,
        #[arg(long)]
        remove: bool,
    },
    /// allow one exact sender owner/device to resolve and send to a named inbox
    GrantInbox {
        address: String,
        #[arg(long)]
        sender_owner: String,
        #[arg(long)]
        sender_device: String,
    },
    /// permanently revoke a cross-org inbox grant by ID
    RevokeInboxGrant { grant_id: String },
    /// resolve a LOCAL@SCOPE inbox address to its verified recipient bundle
    ResolveInbox { address: String },
}

#[derive(Subcommand)]
enum ThreadCmd {
    /// create a stable two-member thread from reciprocal inbox grants
    Create {
        #[arg(long = "from")]
        from_address: String,
        #[arg(long = "to")]
        to_address: String,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// show immutable thread membership and stored message count
    Show {
        #[arg(long)]
        creator_owner: String,
        #[arg(long)]
        thread_id: String,
    },
    /// store one text message; retries reuse the encrypted local request
    Send {
        #[arg(long)]
        creator_owner: String,
        #[arg(long)]
        thread_id: String,
        text: String,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// read one bounded history page addressed to this authenticated device
    Read {
        #[arg(long)]
        creator_owner: String,
        #[arg(long)]
        thread_id: String,
        #[arg(long, default_value_t = 0)]
        after_seq: u64,
        #[arg(long, default_value_t = 20)]
        limit: u8,
    },
}

#[derive(Subcommand)]
enum AccessCmd {
    /// show current local access and permission settings
    Show,
    /// update mode, outbound cross-org send, or max hop settings
    Set {
        #[arg(long, value_parser = ["deny", "same-account", "allowlist"])]
        mode: Option<String>,
        #[arg(long, value_parser = ["on", "off"])]
        external_send: Option<String>,
        #[arg(long)]
        max_hops: Option<i64>,
    },
    /// add a verified sender to the local allowlist (and optionally sync a relay grant)
    Allow {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        device: String,
        #[arg(long)]
        address: Option<String>,
        #[arg(long)]
        label: Option<String>,
        /// also push grant-inbox to the hosted relay (requires --address)
        #[arg(long)]
        sync: bool,
    },
    /// block a sender org or specific device locally
    Deny {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        device: Option<String>,
        #[arg(long)]
        reason: Option<String>,
    },
    /// remove a sender org or device from local allow/block lists
    Remove {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        device: Option<String>,
    },
    /// reset local access settings to safe defaults
    Reset,
}

#[derive(Subcommand)]
enum IdentityCmd {
    /// verify existing auth and explain mapping; never register, grant, send or wake
    Diagnose {
        /// existing gateway config; otherwise diagnose the native account
        #[arg(long, requires = "credentials_stdin")]
        gateway_config: Option<std::path::PathBuf>,
        /// read access_token and identity_handle JSON from stdin, never command arguments
        #[arg(long, requires = "gateway_config")]
        credentials_stdin: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TunnelCmd {
    /// run tunnel-client and restart it when polls stop recovering
    Watch {
        /// profile created by `tunnel-client init`
        #[arg(long, default_value = "everett-dot")]
        profile: String,
        /// tunnel-client binary
        #[arg(long, default_value = "tunnel-client")]
        tunnel_client: std::path::PathBuf,
        /// owner-only file that sets CONTROL_PLANE_API_KEY
        #[arg(long)]
        env_file: std::path::PathBuf,
        /// consecutive poll-timeout log lines without recovery before a restart
        #[arg(long, default_value_t = 3)]
        timeout_streak: u32,
        /// seconds to wait before starting tunnel-client again
        #[arg(long, default_value_t = 2)]
        restart_delay: u64,
        /// stop after this many restarts (0 keeps going)
        #[arg(long, default_value_t = 0)]
        max_restarts: u32,
        /// renew connections of this age unless local health proves fresh polling
        #[arg(long, default_value_t = 240)]
        max_connection_age: u64,
        /// exact gateway OAuth resource; require its tunnel ID in the runtime env file
        #[arg(long)]
        expected_resource: Option<String>,
        /// local tunnel-client detailed health; retain fresh polling instead of blind rotation
        #[arg(long)]
        health_url: Option<String>,
    },
}

#[derive(Subcommand)]
enum ExternalCmd {
    /// add an inbox registration (does not connect to a provider)
    Add {
        #[arg(long)]
        id: String,
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(everett::harness::EXTERNAL.to_vec()))]
        harness: String,
        #[arg(long, default_value = "")]
        title: String,
        #[arg(long, default_value = "")]
        cwd: String,
        /// bind this inbox to a relay peer target (PEER/HARNESS/SESSION) so sends
        /// travel the durable encrypted relay instead of the local inbox file
        #[arg(long)]
        device: Option<String>,
        /// update an existing registration of the same harness
        #[arg(long)]
        replace: bool,
    },
    /// list registrations as JSON
    List,
    /// remove a registration without deleting its inbox
    Remove {
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
enum ProofCmd {
    /// send a nonce canary through ping and wait for its correlated inbox reply
    Wake {
        #[arg(long, value_name = "SESSION")]
        to: String,
        #[arg(long, default_value_t = 120.0)]
        timeout: f64,
        #[arg(long)]
        json: bool,
        /// resolve the target and transport without sending anything
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() {
    let cli = Cli::from_arg_matches(&command().get_matches()).unwrap_or_else(|e| e.exit());
    let mut a = Args {
        hours: cli.hours,
        ..Default::default()
    };
    let cmd = match cli.cmd {
        Some(cmd) => cmd,
        None => {
            use std::io::IsTerminal;
            if everett::onboard::first_run_needed()
                && std::io::stdin().is_terminal()
                && std::io::stdout().is_terminal()
            {
                std::process::exit(everett::onboard::run(&everett::onboard::Options {
                    span_days: 3,
                    ..Default::default()
                }));
            }
            command().print_long_help().unwrap_or_else(|e| {
                eprintln!("everett: {e}");
                std::process::exit(1);
            });
            println!();
            return;
        }
    };
    let code = match cmd {
        Cmd::Proof { action: ProofCmd::Wake { to, timeout, json, dry_run } } => {
            a.to = Some(to);
            a.timeout = timeout;
            a.json = json;
            a.dry_run = dry_run;
            cli::cmd_proof_wake(&a)
        }
        Cmd::Devices {
            action: Some(Devices::Revoke { device_id }),
            ..
        } => peer_output(everett::relay::revoke_device(&device_id)),
        Cmd::Devices { action: None, .. } => peer_output(everett::relay::devices()),
        Cmd::Relay { action } => match action {
            Some(RelayCmd::Diagnose) => peer_output(Ok(everett::relay::diagnostics())),
            Some(RelayCmd::Authors) => peer_output(everett::messaging::authors().map(|sessions| serde_json::json!({"sessions":sessions}))),
            Some(RelayCmd::Expose { session, harness, remove }) => peer_output(everett::messaging::expose_id(&session, harness.as_deref(), remove)),
            None => peer_output(everett::relay::run()),
            Some(RelayCmd::Register) => peer_output(everett::relay::register()),
            Some(RelayCmd::Devices { names: true, .. }) => match everett::relay::device_names() {
                Ok(text) => {
                    print!("{text}");
                    0
                }
                Err(e) => {
                    eprintln!("everett: {e}");
                    e.code
                }
            },
            Some(RelayCmd::Devices { .. }) => peer_output(everett::relay::devices()),
            Some(RelayCmd::Rename { name }) => peer_output(everett::relay::rename_device(&name)),
            Some(RelayCmd::Webhook { session, harness, url_file, bearer_file, cooldown_seconds }) => peer_output(
                everett::receiver_webhook::configure_inbox(&session, &harness, &url_file, &bearer_file, cooldown_seconds)
            ),
            Some(RelayCmd::TrustAccount { session, harness, dry_run }) => peer_output(
                everett::receiver_webhook::trust_account_inbox(&session, &harness, dry_run)
            ),
            Some(RelayCmd::Whoami) => peer_output(everett::relay::admin_call(
                "relay:whoami",
                serde_json::json!({}),
                true,
            )),
            Some(RelayCmd::Rotate) => peer_output(everett::relay::rotate()),
            Some(RelayCmd::RevokeAgent { device }) => peer_output(everett::relay::admin_call(
                "relay:revokeAgent",
                serde_json::json!({"deviceId":device}),
                false,
            )),
            Some(RelayCmd::RevokeMember { subject }) => peer_output(everett::relay::admin_call(
                "relay:revokeMember",
                serde_json::json!({"subject":subject}),
                false,
            )),
            Some(RelayCmd::RegisterInbox { address, harness, session, device, remove }) => peer_output(
                everett::relay::register_inbox(&address, &harness, &session, device.as_deref(), remove)
            ),
            Some(RelayCmd::GrantInbox { address, sender_owner, sender_device }) => peer_output(
                everett::relay::grant_inbox(&address, &sender_owner, &sender_device)
            ),
            Some(RelayCmd::RevokeInboxGrant { grant_id }) => peer_output(
                everett::relay::revoke_inbox_grant(&grant_id)
            ),
            Some(RelayCmd::ResolveInbox { address }) => peer_output(
                everett::relay::resolve_inbox(&address)
            ),
        },
        Cmd::Thread { action } => match action {
            ThreadCmd::Create { from_address, to_address, idempotency_key } => peer_output(
                everett::relay::thread_create(&from_address, &to_address, idempotency_key.as_deref())
            ),
            ThreadCmd::Show { creator_owner, thread_id } => peer_output(
                everett::relay::thread_show(&creator_owner, &thread_id)
            ),
            ThreadCmd::Send { creator_owner, thread_id, text, idempotency_key } => peer_output(
                everett::relay::thread_send(&creator_owner, &thread_id, &text, idempotency_key.as_deref())
            ),
            ThreadCmd::Read { creator_owner, thread_id, after_seq, limit } => peer_output(
                everett::relay::thread_read(&creator_owner, &thread_id, after_seq, limit)
            ),
        },
        Cmd::Peer { action } => match action {
            PeerCmd::AddRelay {
                name,
                device,
                fingerprint,
            } => peer_output(everett::relay::add(
                &name,
                &device,
                &fingerprint,
            )),
            PeerCmd::Add {
                name,
                ssh,
                port,
                binary,
                host_key,
                device_key,
            } => peer_output(
                everett::peer::discover_address(&name, ssh.as_deref(), port).and_then(
                    |(address, port)| {
                        everett::peer::add(&name, &address, port, &binary, &host_key, &device_key)
                    },
                ),
            ),
            PeerCmd::List => peer_output(everett::peer::listing()),
            PeerCmd::Identity => peer_output(everett::peer::identity()),
            PeerCmd::Remove { name, transport } => {
                peer_output(everett::peer::remove_scoped(&name, transport.as_deref()))
            }
            PeerCmd::Revoke { client } => peer_output(everett::peer::revoke(&client)),
            PeerCmd::Card { target } => peer_output(everett::peer::card(&target)),
            PeerCmd::Status { name, message_id } => {
                peer_output(everett::peer::status(&name, &message_id))
            }
            PeerCmd::Serve { client } => {
                everett::peer::serve(client.as_deref(), false)
            }
            PeerCmd::Install => everett::peer::serve(None, true),
            PeerCmd::Ls {
                name,
                all,
                harness,
                json,
            } => {
                a.peer = Some(name);
                a.all = all;
                a.harness = harness;
                a.json = json;
                cli::cmd_ls(&a)
            }
        },
        Cmd::External { action } => match action {
            ExternalCmd::Add {
                id,
                harness,
                title,
                cwd,
                device,
                replace,
            } => cli::cmd_external_add(&id, &harness, &title, &cwd, device.as_deref(), replace),
            ExternalCmd::List => cli::cmd_external_list(),
            ExternalCmd::Remove { id } => cli::cmd_external_remove(&id),
        },
        Cmd::Control { action: ControlCmd::Read { thread, session, messages } } => {
            cli::cmd_control_read(
                thread.as_deref().unwrap_or_default(),
                session.as_deref(),
                messages,
                a.hours,
            )
        }
        Cmd::Control { action: ControlCmd::Threads { project } } => {
            cli::cmd_control_threads(project.as_deref().unwrap_or_default())
        }
        Cmd::Ls {
            json,
            all,
            harness,
            peer,
        } => {
            a.peer = peer;
            a.json = json;
            a.all = all;
            a.harness = harness;
            cli::cmd_ls(&a)
        }
        Cmd::Cards {
            regenerate_auto,
            dry_run,
            hours,
        } => {
            a.regenerate_auto = regenerate_auto;
            a.dry_run = dry_run;
            a.regen_hours = hours;
            cli::cmd_cards(&a)
        }
        Cmd::Skills { task: Some(task), json, .. } => cli::cmd_skill_match(&task, json),
        Cmd::Skills { shortlist: true, json, .. } => cli::cmd_skill_shortlist(json),
        Cmd::Skills { query, get, source, json, .. } => cli::cmd_skills(query.as_deref(), get.as_deref(), source.as_deref(), json),
        Cmd::Dashboard { port, no_open } => everett::dashboard::serve(a.hours, port, no_open),
        Cmd::Route { text, json, router } => {
            a.text = Some(text);
            a.json = json;
            a.router = router;
            cli::cmd_route(&a)
        }
        Cmd::Send {
            text,
            json,
            dry_run,
            timeout,
            router,
            to,
            spawn,
            dir,
            harness,
            mode,
            wait,
            wake,
            idempotency_key,
        } => {
            a.idempotency_key = idempotency_key;
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
            a.wake = wake;
            cli::cmd_send(&a)
        }
        Cmd::Event {
            kind,
            message,
            project,
            session,
        } => {
            a.kind = Some(kind);
            a.message = Some(message);
            a.project = project;
            a.session = session;
            cli::cmd_event(&a)
        }
        Cmd::Events {
            since,
            session,
            json,
            check,
        } => {
            a.since = since;
            a.session = session;
            a.json = json;
            a.check = check;
            cli::cmd_events(&a)
        }
        Cmd::Subscribe {
            target,
            as_,
            remove,
        } => {
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
        Cmd::Sent { to, json } => cli::cmd_sent(to.as_deref(), json),
        Cmd::Status { message_id, json } => cli::cmd_status(&message_id, json),
        Cmd::Retry { message_id, json } => cli::cmd_retry(&message_id, json),
        Cmd::Inbox {
            session,
            peek,
            message,
            reply_to,
            json,
        } => {
            a.session = session;
            a.peek = peek;
            a.message = message;
            a.reply_to = reply_to;
            a.json = json;
            cli::cmd_inbox(&a)
        }
        Cmd::Trunk {
            action,
            dry_run,
            llm,
            at,
            apply,
            remove,
        } => {
            a.action = Some(action);
            a.dry_run = dry_run;
            a.llm = llm;
            a.at = at;
            a.apply = apply;
            a.remove = remove;
            cli::cmd_trunk(&a)
        }
        Cmd::Learn {
            ttl_seconds,
            fact,
            project,
            scope,
            key,
        } => {
            a.fact = Some(fact);
            a.project = project;
            a.scope = scope;
            cli::cmd_learn(&a, ttl_seconds, key.as_deref())
        }
        Cmd::Recall {
            query,
            project,
            scope,
            limit,
            json,
            explain,
        } => cli::cmd_recall(
            &query,
            project.as_deref(),
            scope.as_deref(),
            limit as usize,
            json,
            explain,
        ),
        Cmd::Core { action, project } => {
            if let Some(CoreCmd::Search { query, json }) = action {
                cli::cmd_core_search(&query, project.as_deref(), json)
            } else {
                a.action = match action {
                Some(CoreCmd::EditPath) => Some("edit-path".into()),
                Some(CoreCmd::History) => Some("history".into()),
                _ => None,
                };
                a.project = project;
                cli::cmd_core(&a)
            }
        }
        Cmd::InstallHooks {
            claude,
            codex,
            omp,
            grok,
            pi,
            devin,
            apply,
        } => {
            a.claude = claude;
            a.codex = codex;
            a.omp = omp;
            a.grok = grok;
            a.pi = pi;
            a.devin = devin;
            a.apply = apply;
            cli::cmd_install_hooks(&a)
        }
        Cmd::GatewayWorker => everett::gateway::run_internal_worker(),
        Cmd::Mcp { harness } => everett::mcp::serve(harness.as_deref()),
        Cmd::Tunnel { action } => match action {
            TunnelCmd::Watch {
                profile,
                tunnel_client,
                env_file,
                timeout_streak,
                restart_delay,
                max_restarts,
                max_connection_age,
                expected_resource,
                health_url,
            } => everett::tunnel::run(everett::tunnel::WatchConfig {
                profile,
                tunnel_client,
                env_file,
                timeout_streak,
                restart_delay: std::time::Duration::from_secs(restart_delay),
                max_restarts,
                max_connection_age: std::time::Duration::from_secs(max_connection_age),
                expected_resource,
                health_url,
            }),
        },

        Cmd::Gateway {
            config,
            transport,
            port,
        } => everett::gateway::run(&config, &transport, port),
        Cmd::Access { config, json, action } => {
            let outcome = match action.unwrap_or(AccessCmd::Show) {
                AccessCmd::Show => everett::access::show(config.as_deref()).map(|(p, pol)| (p, pol, None)),
                AccessCmd::Set { mode, external_send, max_hops } => {
                    let ext = external_send.as_deref().map(|v| v == "on");
                    everett::access::set_settings(config.as_deref(), mode.as_deref(), ext, max_hops)
                        .map(|(p, pol)| (p, pol, None))
                }
                AccessCmd::Allow { owner, device, address, label, sync } => everett::access::allow_sender(
                    config.as_deref(),
                    &owner,
                    &device,
                    address.as_deref(),
                    label.as_deref(),
                    sync,
                ),
                AccessCmd::Deny { owner, device, reason } => everett::access::deny_sender(
                    config.as_deref(),
                    &owner,
                    device.as_deref(),
                    reason.as_deref(),
                )
                .map(|(p, pol)| (p, pol, None)),
                AccessCmd::Remove { owner, device } => {
                    everett::access::remove_sender(config.as_deref(), &owner, device.as_deref())
                        .map(|(p, pol)| (p, pol, None))
                }
                AccessCmd::Reset => everett::access::reset(config.as_deref()).map(|(p, pol)| (p, pol, None)),
            };
            match outcome {
                Ok((path, policy, grant)) => {
                    println!("{}", everett::access::format_output(&path, &policy, grant.as_ref(), json));
                    0
                }
                Err(error) => {
                    eprintln!("everett: {error}");
                    error.code
                }
            }
        }
        Cmd::GatewayPermissions { file, all, allow, deny_all } =>
            everett::gateway::permissions::run(&file, all, allow, deny_all),
        Cmd::InstallMcp {
            claude,
            codex,
            omp,
            grok,
            devin,
            pi,
            hermes,
            apply,
            repair,
        } => {
            a.claude = claude;
            a.codex = codex;
            a.omp = omp;
            a.grok = grok;
            a.devin = devin;
            a.pi = pi;
            a.hermes = hermes;
            a.apply = apply;
            a.repair = repair;
            cli::cmd_install_mcp(&a)
        }
        Cmd::Login {
            provider, issuer, base_url, cancel, no_wait,
            device,
            open,
            force,
            json,
        } => cli::cmd_login(
            provider.as_deref(),
            &everett::cloudflare_auth::LoginOptions { issuer, base_url, cancel, no_wait, device, open, force },
            json,
        ),
        Cmd::Identity { action: IdentityCmd::Diagnose { gateway_config, credentials_stdin: _, json } } => {
            everett::gateway::diagnostics::run(gateway_config.as_deref(), json)
        }
        Cmd::Whoami { json, provider } => cli::cmd_whoami(provider.as_deref(), json),
        Cmd::Logout { provider, json, revoke_device } => cli::cmd_logout(provider.as_deref(), revoke_device, json),
        Cmd::Doctor => everett::doctor::run(a.hours),
        Cmd::Onboard {
            handoff,
            handoff_preview,
            yes,
            span_days,
            no_backfill,
            no_mcp,
            jev_key_env,
            schedule_merge,
        } => {
            if handoff.is_some() || handoff_preview {
                let conflicting_options =
                    yes || span_days.is_some() || no_backfill || no_mcp || jev_key_env.is_some() || schedule_merge;
                everett::onboard_handoff::run(handoff.as_deref(), handoff_preview, conflicting_options)
            } else {
                everett::onboard::run(&everett::onboard::Options {
                    yes, span_days: span_days.unwrap_or(3), no_backfill, no_mcp, jev_key_env, schedule_merge,
                })
            }
        }
        Cmd::Hook { stem, args } => everett::hooks_common::hook_main(&stem, &args),
    };
    std::process::exit(code);
}
