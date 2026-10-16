use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use wait_timeout::ChildExt;

use crate::error::{EverettError, Result};
use crate::{inbox, registry, send, session};

pub const BODY_CAP: usize = 64 * 1024;
const RESPONSE_CAP: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Serialize, Deserialize)]
pub struct Peer {
    pub address: String,
    pub port: u16,
    pub device_id: String,
    pub host_fingerprint: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sender {
    session: String,
    harness: String,
    card: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Hello,
    Ls {
        hours: f64,
        all: bool,
        harness: String,
    },
    Card {
        session: String,
        harness: String,
    },
    Send {
        session: String,
        harness: String,
        text: String,
        from: Sender,
        idempotency_key: String,
        hops: i64,
    },
    Status {
        message_id: String,
    },
    Revoke,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Install {
    client_id: String,
    public_key: String,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    sender: String,
    harness: String,
    session: String,
}

fn bad(message: impl Into<String>) -> EverettError {
    EverettError::new(2, message)
}

fn io_error(error: std::io::Error) -> EverettError {
    bad(format!("peer storage: {error}"))
}

fn root() -> PathBuf {
    session::home().join(".everett/peer")
}

pub(crate) fn private_dir(path: &Path) -> Result<()> {
    private_dir_io(path).map_err(io_error)
}

/// Persist each new directory's link in its parent, not just its contents.
/// Directory fsync is supported on both Linux and macOS; no Linux-only flags.
pub(crate) fn private_dir_io(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if !path.is_dir() {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| std::io::Error::other("missing directory parent"))?;
        if !parent.is_dir() {
            private_dir_io(parent)?;
        }
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => (),
            Err(e) => return Err(e),
        }
    }
    let mode = fs::metadata(path)?.permissions().mode() & 0o700;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    File::open(path)?.sync_all()?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn save(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().ok_or_else(|| bad("missing parent"))?)
            .map_err(io_error)?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(io_error)?;
    temp.write_all(bytes).map_err(io_error)?;
    temp.as_file().sync_all().map_err(io_error)?;
    temp.persist(path).map_err(|e| io_error(e.error))?;
    Ok(())
}

pub(crate) fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(io_error)?;
    file.lock_exclusive().map_err(io_error)?;
    Ok(file)
}

/// One ownership lock covers the SSH and Convex alias registries. Holding it
/// across the final conflict check and commit prevents either transport from
/// silently shadowing the other.
fn alias_lock_at(shared: &Path) -> Result<File> {
    private_dir(shared)?;
    lock(&shared.join("peer-alias.lock"))
}

pub(crate) fn alias_lock() -> Result<File> {
    alias_lock_at(&session::home().join(".everett"))
}

/// Atomically check the other transport and commit this transport's alias.
/// Both SSH and Convex use this path so neither can win a check-then-write race.
pub(crate) fn claim_alias<T>(
    name: &str,
    transport: &str,
    other_configured: impl FnOnce() -> Result<bool>,
    commit: impl FnOnce() -> Result<T>,
) -> Result<T> {
    claim_alias_at(
        &session::home().join(".everett"),
        name,
        transport,
        other_configured,
        commit,
    )
}

fn claim_alias_at<T>(
    shared: &Path,
    name: &str,
    transport: &str,
    other_configured: impl FnOnce() -> Result<bool>,
    commit: impl FnOnce() -> Result<T>,
) -> Result<T> {
    valid(name)?;
    let _guard = alias_lock_at(shared)?;
    if other_configured()? {
        return Err(bad(format!(
            "alias already selects {transport}; use a different alias"
        )));
    }
    commit()
}

pub(crate) fn assert_alias_unambiguous(name: &str) -> Result<()> {
    valid(name)?;
    if config_path(name)?.exists() && crate::relay::configured(name) {
        return Err(bad(
            "alias is configured for both SSH and Convex; remove one binding explicitly before use",
        ));
    }
    Ok(())
}

fn valid(value: &str) -> Result<()> {
    if !inbox::valid_id(value) {
        return Err(bad(format!("invalid peer identifier: {value:?}")));
    }
    Ok(())
}

fn config_path(name: &str) -> Result<PathBuf> {
    valid(name)?;
    Ok(root().join(format!("{name}.json")))
}

fn known_hosts(name: &str) -> Result<PathBuf> {
    valid(name)?;
    Ok(root().join(format!("{name}.known_hosts")))
}

pub fn peers() -> Result<BTreeMap<String, Peer>> {
    if !root().exists() {
        return Ok(BTreeMap::new());
    }
    let mut peers = BTreeMap::new();
    for file in crate::paths::glob(&root(), "*.json") {
        let name = file
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let peer = serde_json::from_slice(&fs::read(&file).map_err(io_error)?)
            .map_err(|e| bad(format!("invalid peer config {}: {e}", file.display())))?;
        peers.insert(name, peer);
    }
    Ok(peers)
}

fn load(name: &str) -> Result<Peer> {
    let path = config_path(name)?;
    let raw =
        fs::read(path).map_err(|_| bad(format!("unknown peer {name:?}; use everett peer add")))?;
    serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid peer config: {e}")))
}

pub fn listing() -> Result<Value> {
    let mut list: serde_json::Map<String, Value> = peers()?
        .into_iter()
        .map(|(name, peer)| serde_json::to_value(peer).map(|value| (name, value)))
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| bad(e.to_string()))?;
    let relay = crate::relay::list()?;
    let relay = relay
        .as_object()
        .ok_or_else(|| bad("invalid relay peer list"))?
        .clone();
    for name in relay.keys() {
        if list.contains_key(name) {
            return Err(bad("alias is configured for both SSH and Convex; remove one binding explicitly before use"));
        }
    }
    list.extend(relay);
    Ok(Value::Object(list))
}

fn run(mut command: Command, input: Vec<u8>, cap: usize) -> Result<Vec<u8>> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| EverettError::new(7, format!("peer unreachable: {e}")))?;
    let pid = child.id() as i32;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take((cap + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(8192).read_to_end(&mut bytes).map(|_| bytes)
    });
    let status = child.wait_timeout(TIMEOUT).map_err(io_error)?;
    // Kill descendants too: a wrapper must not keep our pipe readers alive after SSH exits.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    if status.is_none() {
        let _ = child.wait();
    }
    let written = writer.join().map_err(|_| bad("peer stdin writer failed"))?;
    let bytes = output
        .join()
        .map_err(|_| bad("peer stdout reader failed"))?
        .map_err(io_error)?;
    let errors = errors
        .join()
        .map_err(|_| bad("peer stderr reader failed"))?
        .map_err(io_error)?;
    match status {
        None => Err(EverettError::new(
            7,
            "peer unreachable: SSH request timed out after 20 seconds; no local queue",
        )),
        Some(status) if !status.success() => Err(EverettError::new(
            7,
            format!(
                "peer unreachable or SSH host-key mismatch: {}; no local queue",
                String::from_utf8_lossy(&errors).trim()
            ),
        )),
        Some(_) if bytes.len() > cap => Err(bad("peer response exceeds size limit")),
        Some(_) => {
            written.map_err(io_error)?;
            Ok(bytes)
        }
    }
}

fn key_fingerprint(path: &Path) -> Result<String> {
    let mut cmd = Command::new("ssh-keygen");
    cmd.args(["-l", "-E", "sha256", "-f"]).arg(path);
    let output = run(cmd, Vec::new(), 8192)?;
    let text = String::from_utf8_lossy(&output);
    let lines: Vec<_> = text.lines().collect();
    if lines.len() != 1 {
        return Err(bad("expected exactly one pinned Ed25519 host key"));
    }
    lines[0]
        .split_whitespace()
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| bad("invalid SSH fingerprint"))
}

pub fn identity() -> Result<Value> {
    private_dir(&root())?;
    let _guard = lock(&root().join("identity.lock"))?;
    let id_path = root().join("device_id");
    let device_id = if id_path.exists() {
        fs::read_to_string(&id_path).map_err(io_error)?
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        save(&id_path, id.as_bytes())?;
        id
    };
    valid(&device_id)?;
    let key = root().join("id_ed25519");
    if !key.exists() {
        let mut cmd = Command::new("ssh-keygen");
        cmd.args(["-q", "-t", "ed25519", "-N", "", "-C"])
            .arg(format!("everett-peer:{device_id}"))
            .arg("-f")
            .arg(&key);
        run(cmd, Vec::new(), 8192)?;
    }
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).map_err(io_error)?;
    let public_key = fs::read_to_string(key.with_extension("pub")).map_err(io_error)?;
    Ok(
        json!({"device_id": device_id, "public_key": public_key.trim(), "fingerprint": key_fingerprint(&key)?}),
    )
}

fn address_valid(address: &str) -> Result<()> {
    if address.is_empty()
        || address.starts_with('-')
        || address.split('@').count() > 2
        || !address
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._-:[]".contains(&b))
    {
        return Err(bad(
            "--ssh must be a literal [user@]host, not a shell expression or SSH alias",
        ));
    }
    Ok(())
}

pub(crate) fn parse_registry_address(value: &str) -> Result<(String, u16)> {
    let pattern = regex::Regex::new(
        r"^([A-Za-z0-9_.-]+@(?:[A-Za-z0-9_.-]+|\[[0-9a-fA-F:]+\]))(?::([0-9]{1,5}))?$",
    )
    .unwrap();
    let parts = pattern
        .captures(value)
        .filter(|_| value.len() <= 512)
        .ok_or_else(|| bad("SSH registry address must be user@host[:port]; bracket IPv6 hosts"))?;
    let address = parts[1].to_string();
    address_valid(&address)?;
    let port = parts
        .get(2)
        .map_or(Ok(22), |p| p.as_str().parse::<u16>())
        .map_err(|_| bad("SSH registry port must be between 1 and 65535"))?;
    if port == 0 {
        return Err(bad("SSH registry port must be between 1 and 65535"));
    }
    Ok((address, port))
}

pub fn discover_address(name: &str, ssh: Option<&str>, port: Option<u16>) -> Result<(String, u16)> {
    valid(name)?;
    let configured = crate::auth::load().is_some()
        && !crate::config::get("relay.url", Some("EVERETT_CONVEX_URL"), "").is_empty();
    let discovered = if ssh.is_none() || configured {
        crate::device_registry::resolve(name)?
            .map(|address| parse_registry_address(&address))
            .transpose()?
    } else {
        None
    };
    if let Some(address) = ssh {
        address_valid(address)?;
        let port = port.unwrap_or(22);
        if discovered
            .as_ref()
            .is_some_and(|(found, found_port)| found != address || *found_port != port)
        {
            return Err(bad("explicit --ssh/--port conflicts with the registry address; verify the device address before pairing"));
        }
        return Ok((address.into(), port));
    }
    let (address, found_port) = discovered
        .ok_or_else(|| bad("device has no published SSH address; use --ssh user@host"))?;
    if port.is_some_and(|port| port != found_port) {
        return Err(bad("--port conflicts with the registry address"));
    }
    Ok((address, found_port))
}

fn ssh(peer: &Peer, pin: &Path, restricted: bool, first: bool) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-F",
        "/dev/null",
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "ConnectionAttempts=1",
        "-o",
        "GlobalKnownHostsFile=/dev/null",
        "-o",
        "HostKeyAlgorithms=ssh-ed25519",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ForwardX11=no",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "ControlMaster=no",
        "-o",
        "ControlPath=none",
    ]);
    cmd.arg("-o")
        .arg(format!("UserKnownHostsFile={}", pin.display()));
    cmd.arg("-o").arg(if first {
        "StrictHostKeyChecking=accept-new"
    } else {
        "StrictHostKeyChecking=yes"
    });
    if restricted {
        cmd.args(["-o", "IdentitiesOnly=yes", "-o", "IdentityAgent=none", "-i"])
            .arg(root().join("id_ed25519"));
    }
    cmd.arg("-p")
        .arg(peer.port.to_string())
        .arg("--")
        .arg(&peer.address);
    cmd
}

fn frame(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).map_err(|e| bad(e.to_string()))?;
    bytes.push(b'\n');
    if bytes.len() > BODY_CAP {
        return Err(bad("peer request exceeds 64 KiB"));
    }
    Ok(bytes)
}

fn response(bytes: &[u8]) -> Result<Value> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| bad(format!("invalid peer response: {e}")))?;
    if let Some(error) = value.get("error") {
        return Err(EverettError::new(
            error["code"].as_i64().unwrap_or(2) as i32,
            error["message"].as_str().unwrap_or("peer request failed"),
        ));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| bad("peer response missing result"))
}

fn rpc(name: &str, request: &Request) -> Result<Value> {
    let input = frame(request)?;
    let peer = load(name)?;
    let pin = known_hosts(name)?;
    if key_fingerprint(&pin)? != peer.host_fingerprint {
        return Err(bad("pinned SSH host-key mismatch; remove and explicitly re-pair after verifying the new key"));
    }
    response(&run(ssh(&peer, &pin, true, false), input, RESPONSE_CAP)?)
}

pub fn add(
    name: &str,
    address: &str,
    port: u16,
    binary: &str,
    fingerprint: &str,
    device_fingerprint: &str,
) -> Result<Value> {
    valid(name)?;
    address_valid(address)?;
    if port == 0 || !Path::new(binary).is_absolute() || binary.chars().any(char::is_control) {
        return Err(bad(
            "use a nonzero port and --binary with the remote binary's absolute path",
        ));
    }
    if !fingerprint.starts_with("SHA256:") || fingerprint.split_whitespace().count() != 1 {
        return Err(bad(
            "--host-key requires the SHA256 fingerprint verified on the remote machine",
        ));
    }
    // Claim the alias before generating identity, probing/pinning hosts or
    // installing a remote key. The same lock remains held through commit.
    claim_alias(
        name,
        "Convex",
        || Ok(crate::relay::configured(name)),
        || {
            let ident = identity()?;
            let _guard = lock(&root().join("pairing.lock"))?;
            let pin = known_hosts(name)?;
            let temporary = tempfile::NamedTempFile::new_in(root()).map_err(io_error)?;
            let mut peer = Peer {
                address: address.into(),
                port,
                device_id: String::new(),
                host_fingerprint: fingerprint.into(),
            };
            let mut expected_device = None;
            if config_path(name)?.exists() {
                let existing = load(name)?;
                if existing.address != address
                    || existing.port != port
                    || existing.host_fingerprint != fingerprint
                {
                    return Err(bad(
                "peer already pinned; remove and explicitly re-pair to change its address or key",
            ));
                }
                expected_device = Some(existing.device_id);
            }
            if pin.exists() {
                if key_fingerprint(&pin)? != fingerprint {
                    return Err(bad("pinned SSH host-key mismatch"));
                }
                fs::copy(&pin, temporary.path()).map_err(io_error)?;
            }
            let first = !pin.exists();
            let mut probe = ssh(&peer, temporary.path(), false, first);
            probe.arg("true");
            run(probe, Vec::new(), 8192)?;
            if key_fingerprint(temporary.path())? != fingerprint {
                return Err(bad(
                    "SSH host-key fingerprint mismatch; pairing refused before key installation",
                ));
            }
            save(&pin, &fs::read(temporary.path()).map_err(io_error)?)?;
            let mut inspect = ssh(&peer, &pin, false, false);
            inspect.arg(format!(
                "{} peer identity",
                crate::proc::shlex_join(&[binary.into()])
            ));
            let remote_identity: Value = serde_json::from_slice(&run(inspect, Vec::new(), 8192)?)
                .map_err(|e| bad(e.to_string()))?;
            if remote_identity["fingerprint"] != device_fingerprint {
                return Err(bad(
            "Everett device-key fingerprint mismatch; pairing refused before key installation",
        ));
            }
            let request = Install {
                client_id: ident["device_id"].as_str().unwrap().into(),
                public_key: ident["public_key"].as_str().unwrap().into(),
            };
            let mut setup = ssh(&peer, &pin, false, false);
            setup.arg(format!(
                "{} peer install",
                crate::proc::shlex_join(&[binary.into()])
            ));
            let installed = response(&run(setup, frame(&request)?, RESPONSE_CAP)?)?;
            peer.device_id = installed["device_id"]
                .as_str()
                .ok_or_else(|| bad("peer did not return a device id"))?
                .into();
            valid(&peer.device_id)?;
            if remote_identity["device_id"] != peer.device_id {
                return Err(bad("remote Everett device identity changed during pairing"));
            }
            if expected_device
                .as_ref()
                .is_some_and(|id| id != &peer.device_id)
            {
                return Err(bad(
                    "remote Everett device identity changed; verify and explicitly re-pair",
                ));
            }
            let hello = response(&run(
                ssh(&peer, &pin, true, false),
                frame(&Request::Hello)?,
                RESPONSE_CAP,
            )?)?;
            if hello["device_id"] != peer.device_id {
                return Err(bad("peer device identity changed during pairing"));
            }
            save(
                &config_path(name)?,
                &serde_json::to_vec(&peer).map_err(|e| bad(e.to_string()))?,
            )?;
            Ok(
                json!({"peer": name, "device_id": peer.device_id, "host_fingerprint": fingerprint, "replies": "status polling"}),
            )
        },
    )
}

pub fn local_device_id() -> Result<Option<String>> {
    match fs::read_to_string(root().join("device_id")) {
        Ok(id) => {
            valid(&id)?;
            Ok(Some(id))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(e)),
    }
}

fn tag(client: &str) -> String {
    format!("# everett-peer:{client}")
}

fn authorized(client: &str) -> Result<()> {
    valid(client)?;
    let raw = fs::read_to_string(session::home().join(".ssh/authorized_keys")).map_err(io_error)?;
    if !raw.lines().any(|line| line.ends_with(&tag(client))) {
        return Err(bad("peer client is not authorized or has been revoked"));
    }
    Ok(())
}

fn edit_authorized(client: &str, replacement: Option<&str>) -> Result<bool> {
    rewrite_authorized(client, None, replacement)
}

/// Drops lines tagged for `client` (only those carrying `key`, when given), then appends `replacement`.
fn rewrite_authorized(client: &str, key: Option<&str>, replacement: Option<&str>) -> Result<bool> {
    valid(client)?;
    let dir = session::home().join(".ssh");
    private_dir(&dir)?;
    let _guard = lock(&dir.join("everett-peer.lock"))?;
    let file = dir.join("authorized_keys");
    let raw = match fs::read(&file) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(io_error(e)),
    };
    let text = std::str::from_utf8(&raw)
        .map_err(|_| bad("authorized_keys is not UTF-8; refusing to rewrite"))?;
    let mut next: String = text
        .split_inclusive('\n')
        .filter(|line| {
            let line = line.trim_end_matches(['\r', '\n']);
            !(line.ends_with(&tag(client))
                && key.is_none_or(|key| line.split_whitespace().any(|field| field == key)))
        })
        .collect();
    if let Some(line) = replacement {
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(line);
        next.push('\n');
    }
    if next.as_bytes() == raw {
        return Ok(false);
    }
    if !raw.is_empty() {
        let backup = dir.join(format!(
            "authorized_keys.everett-{}.bak",
            uuid::Uuid::new_v4()
        ));
        save(&backup, &raw)?;
    }
    save(&file, next.as_bytes())?;
    Ok(true)
}

pub fn prune_revoked(client: &str) -> Result<bool> {
    edit_authorized(client, None)
}

/// Prunes `client`'s authorization only where it holds this exact SSH key.
pub fn prune_revoked_key(client: &str, key: &str) -> Result<bool> {
    rewrite_authorized(client, Some(key), None)
}

/// This machine's SSH peer id, the base64 field of its public key, and its signing key.
pub fn local_ssh_key() -> Result<Option<(String, String, ed25519_dalek::SigningKey)>> {
    let Some(id) = local_device_id()? else {
        return Ok(None);
    };
    let path = root().join("id_ed25519");
    if !path.exists() {
        return Ok(None);
    }
    let private =
        ssh_key::PrivateKey::read_openssh_file(&path).map_err(|e| bad(format!("SSH identity: {e}")))?;
    let pair = private
        .key_data()
        .ed25519()
        .ok_or_else(|| bad("SSH identity is not Ed25519"))?;
    let public = private
        .public_key()
        .to_openssh()
        .map_err(|e| bad(format!("SSH identity: {e}")))?;
    let field = public
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| bad("invalid SSH public key"))?
        .to_string();
    Ok(Some((
        id,
        field,
        ed25519_dalek::SigningKey::from_bytes(&pair.private.to_bytes()),
    )))
}

pub fn revoke(client: &str) -> Result<Value> {
    edit_authorized(client, None)?;
    Ok(json!({"revoked": client}))
}

fn install(request: Install) -> Result<Value> {
    valid(&request.client_id)?;
    let pieces: Vec<_> = request.public_key.split_whitespace().collect();
    if pieces.len() < 2
        || pieces[0] != "ssh-ed25519"
        || !pieces[1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
    {
        return Err(bad("expected an Ed25519 public key"));
    }
    let ident = identity()?;
    let mut check = tempfile::NamedTempFile::new_in(root()).map_err(io_error)?;
    writeln!(check, "{} {}", pieces[0], pieces[1]).map_err(io_error)?;
    key_fingerprint(check.path())?;
    let binary = fs::canonicalize(std::env::current_exe().map_err(io_error)?).map_err(io_error)?;
    let command = crate::proc::shlex_join(&[
        binary.to_string_lossy().into(),
        "peer".into(),
        "serve".into(),
        "--client".into(),
        request.client_id.clone(),
    ]);
    let escaped = command.replace('\\', "\\\\").replace('"', "\\\"");
    let line = format!(
        "command=\"{escaped}\",no-pty,no-port-forwarding,no-agent-forwarding,no-X11-forwarding {} {} {}",
        pieces[0],
        pieces[1],
        tag(&request.client_id)
    );
    edit_authorized(&request.client_id, Some(&line))?;
    Ok(json!({"device_id": ident["device_id"], "client_id": request.client_id, "binary": binary}))
}

fn read_frame(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::io::BufReader::new(reader.take((BODY_CAP + 1) as u64))
        .read_until(b'\n', &mut bytes)
        .map_err(io_error)?;
    if bytes.len() > BODY_CAP {
        return Err(bad("peer request exceeds 64 KiB"));
    }
    if bytes.last() != Some(&b'\n') {
        return Err(bad(
            "peer request must be one newline-terminated JSON frame",
        ));
    }
    Ok(bytes)
}

pub fn serve(client: Option<&str>, setup: bool) -> i32 {
    serve_with_address(client, setup, None)
}

pub fn serve_with_address(client: Option<&str>, setup: bool, address: Option<&str>) -> i32 {
    let (finished, timeout) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if timeout.recv_timeout(TIMEOUT) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            eprintln!("everett: peer request timed out after 20 seconds");
            std::process::exit(7);
        }
    });
    let result = (|| {
        if !setup {
            authorized(client.ok_or_else(|| {
                bad("peer serve requires the forced command's --client identity")
            })?)?;
            if crate::auth::load().is_some() {
                let address = address.map(str::to_string);
                let (published, publication) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let result = crate::device_registry::declared_address(address.as_deref())
                        .and_then(|address| match address {
                            Some(address) => {
                                crate::relay::register_with_address(Some(&address)).map(|_| ())
                            }
                            None => Ok(()),
                        });
                    let _ = published.send(result);
                });
                match publication.recv_timeout(Duration::from_secs(3)) {
                    Ok(Ok(_)) => (),
                    Ok(Err(error)) => {
                        eprintln!("everett: SSH address publication failed: {}", error.message)
                    }
                    Err(_) => eprintln!(
                        "everett: SSH address publication timed out; retry everett relay register"
                    ),
                }
            }
        }
        let bytes = read_frame(std::io::stdin().lock())?;
        if setup {
            let req = serde_json::from_slice(&bytes)
                .map_err(|e| bad(format!("invalid pairing request: {e}")))?;
            install(req)
        } else {
            let req = serde_json::from_slice(&bytes)
                .map_err(|e| bad(format!("invalid peer request: {e}")))?;
            dispatch(client.unwrap(), req)
        }
    })();
    let value = match result {
        Ok(result) => json!({"result": result}),
        Err(e) => json!({"error": {"code": e.code, "message": e.message}}),
    };
    let bytes = serde_json::to_vec(&value).unwrap();
    if bytes.len() > RESPONSE_CAP {
        println!(
            "{}",
            json!({"error":{"code":2,"message":"peer response exceeds 1 MiB; narrow the listing"}})
        );
    } else {
        println!("{}", String::from_utf8_lossy(&bytes));
    }
    let _ = finished.send(());
    0
}

fn dispatch(client: &str, request: Request) -> Result<Value> {
    match request {
        Request::Hello => Ok(json!({"device_id": identity()?["device_id"]})),
        Request::Revoke => revoke(client),
        Request::Ls {
            hours,
            all,
            harness,
        } => {
            if !hours.is_finite() || hours <= 0.0 {
                return Err(bad("hours must be positive and finite"));
            }
            Ok(json!(registry::scan(
                hours,
                all,
                Some(registry::CAP),
                &harness
            )))
        }
        Request::Card { session, harness } => {
            let target = remote_session(&session, &harness)?;
            Ok(
                json!({"card": target.card, "source": target.card_source, "updated": target.last_active}),
            )
        }
        req @ Request::Send { .. } => receive(client, req),
        Request::Status { message_id } => remote_status(client, &message_id),
    }
}

fn remote_session(id: &str, harness: &str) -> Result<session::Session> {
    valid(id)?;
    registry::scan(24.0 * 365.0, true, None, harness)
        .into_iter()
        .find(|s| s.id == id && s.harness == harness)
        .ok_or_else(|| bad("remote session not found"))
}

fn delivery_lock() -> Result<File> {
    private_dir(&root())?;
    lock(&root().join("delivery.lock"))
}

fn receive(client: &str, request: Request) -> Result<Value> {
    let digest = format!("{:x}", Sha256::digest(frame(&request)?));
    let Request::Send {
        session,
        harness,
        text,
        from,
        idempotency_key,
        hops,
    } = request
    else {
        unreachable!()
    };
    valid(&session)?;
    valid(&from.session)?;
    valid(&idempotency_key)?;
    if !(1..=inbox::MAX_HOPS).contains(&hops) {
        return Err(EverettError::new(
            7,
            "cross-device hop limit reached (maximum 3)",
        ));
    }
    let _guard = delivery_lock()?;
    for file in crate::paths::glob(&inbox::inbox_dir(), "*.jsonl") {
        for item in inbox::read(&file.file_stem().unwrap_or_default().to_string_lossy()) {
            if item.get("peer_client") == Some(&json!(client))
                && item.get("peer_key") == Some(&json!(idempotency_key))
            {
                if item.get("peer_digest") != Some(&json!(digest)) {
                    return Err(bad("idempotency key already used for a different request"));
                }
                return remote_status(client, item["id"].as_str().unwrap());
            }
        }
    }
    let target = remote_session(&session, &harness)?;
    let mut extra = Map::new();
    extra.insert("peer_client".into(), json!(client));
    extra.insert("peer_key".into(), json!(idempotency_key));
    extra.insert("peer_digest".into(), json!(digest));
    extra.insert("peer_sender".into(), json!(from.session));
    let message = inbox::post(
        &target.id,
        &text,
        &format!("{client}/{}/{}", from.harness, from.session),
        "message",
        "",
        hops,
        &from.harness,
        &from.card,
        Some(extra),
    )?;
    Ok(json!({"message_id": message["id"], "state": "queued"}))
}

fn remote_status(client: &str, message_id: &str) -> Result<Value> {
    valid(message_id)?;
    let original = inbox::find(message_id)
        .filter(|m| m.get("peer_client") == Some(&json!(client)))
        .ok_or_else(|| bad("no message for this peer client"))?;
    let reply = inbox::read(&format!("peer-{client}"))
        .into_iter()
        .find(|m| m.get("reply_to") == Some(&json!(message_id)));
    let done = inbox::done_ids(original["to"].as_str().unwrap()).contains(message_id);
    Ok(
        json!({"message_id": message_id, "state": if reply.is_some() { "replied" } else if done { "delivered" } else { "queued" }, "reply": reply}),
    )
}

pub fn retain_reply(
    original: &Map<String, Value>,
    text: &str,
    who: &Map<String, Value>,
    hops: i64,
) -> Result<Map<String, Value>> {
    let sender = who["sender"].as_str().unwrap_or(inbox::HUMAN);
    if inbox::external(original)
        || inbox::relay_context(sender)
            .as_ref()
            .is_some_and(inbox::external)
    {
        return Err(bad("external/untrusted replies require relay or local inbox; SSH cannot preserve this provenance"));
    }
    let client = original["peer_client"]
        .as_str()
        .ok_or_else(|| bad("missing peer client"))?;
    valid(client)?;
    let _guard = delivery_lock()?;
    let destination = format!("peer-{client}");
    let mid = original["id"].as_str().unwrap();
    let message = if let Some(existing) = inbox::read(&destination)
        .into_iter()
        .find(|m| m.get("reply_to") == Some(&json!(mid)))
    {
        if existing.get("text") != Some(&json!(text.trim())) {
            return Err(bad(
                "a different reply is already retained for this message",
            ));
        }
        existing
    } else {
        inbox::post(
            &destination,
            text,
            who["sender"].as_str().unwrap_or(inbox::HUMAN),
            "reply",
            mid,
            hops,
            who["from_harness"].as_str().unwrap_or(""),
            who["from_card"].as_str().unwrap_or(""),
            None,
        )?
    };
    Ok(serde_json::from_value(json!({"mode":"peer-reply", "message_id":message["id"], "reply_to":mid, "to":original["from"], "pickup":"status polling"})).unwrap())
}

pub fn ls(name: &str, hours: f64, all: bool, harness: &str) -> Result<Value> {
    assert_alias_unambiguous(name)?;
    if crate::relay::configured(name) {
        return Err(bad(
            "Convex peers use explicit session IDs; remote ls/card are SSH-only",
        ));
    }
    let mut rows = rpc(
        name,
        &Request::Ls {
            hours,
            all,
            harness: harness.into(),
        },
    )?;
    let rows_array = rows
        .as_array_mut()
        .ok_or_else(|| bad("peer listing is not an array"))?;
    for row in rows_array {
        let row = row
            .as_object_mut()
            .ok_or_else(|| bad("peer listing contains a non-session value"))?;
        let harness = row
            .get("harness")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("peer session missing harness"))?;
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("peer session missing id"))?;
        let prefixed = format!("{name}/{harness}/{id}");
        row.insert("device".into(), json!(name));
        row.insert("id".into(), json!(prefixed));
    }
    Ok(rows)
}

pub fn target(value: &str) -> Result<(&str, &str, &str)> {
    let parts: Vec<_> = value.split('/').collect();
    if parts.len() != 3 {
        return Err(bad("remote target must be PEER/HARNESS/SESSION"));
    }
    for part in &parts {
        valid(part)?;
    }
    Ok((parts[0], parts[1], parts[2]))
}

pub fn card(value: &str) -> Result<Value> {
    let (name, harness, session) = target(value)?;
    assert_alias_unambiguous(name)?;
    if crate::relay::configured(name) {
        return Err(bad(
            "Convex peers use explicit session IDs; remote ls/card are SSH-only",
        ));
    }
    rpc(
        name,
        &Request::Card {
            session: session.into(),
            harness: harness.into(),
        },
    )
}

fn receipt_path(name: &str, message_id: &str) -> Result<PathBuf> {
    valid(name)?;
    valid(message_id)?;
    let dir = root().join("outgoing");
    private_dir(&dir)?;
    Ok(dir.join(format!("{name}-{message_id}.json")))
}

pub fn send(value: &str, text: &str, key: Option<&str>, caller: Option<&str>) -> Result<Value> {
    send_as(value, text, key, caller, None)
}

pub fn send_as(
    value: &str,
    text: &str,
    key: Option<&str>,
    caller: Option<&str>,
    caller_harness: Option<&str>,
) -> Result<Value> {
    let (name, harness, session) = target(value)?;
    assert_alias_unambiguous(name)?;
    if crate::relay::configured(name) {
        return crate::relay::send_as(value, text, key, caller, caller_harness);
    }
    let who = send::sender_info_as(caller, caller_harness);
    let sender = who["sender"].as_str().unwrap_or(inbox::HUMAN);
    if inbox::relay_context(sender)
        .as_ref()
        .is_some_and(inbox::external)
    {
        return Err(bad("external/untrusted forwarding requires relay or local inbox; the independent SSH wire cannot preserve this provenance"));
    }
    let hops = send::caller_hops_for(Some(sender));
    if hops >= inbox::MAX_HOPS {
        return Err(EverettError::new(
            7,
            "cross-device hop limit reached (maximum 3)",
        ));
    }
    let hops = hops.max(0) + 1;
    let generated = uuid::Uuid::new_v4().to_string();
    let key = key.unwrap_or(&generated);
    let result = rpc(
        name,
        &Request::Send {
            session: session.into(),
            harness: harness.into(),
            text: text.into(),
            from: Sender {
                session: sender.into(),
                harness: who["from_harness"].as_str().unwrap_or("").into(),
                card: who["from_card"].as_str().unwrap_or("").into(),
            },
            idempotency_key: key.into(),
            hops,
        },
    )?;
    let mid = result["message_id"]
        .as_str()
        .ok_or_else(|| bad("peer did not return a message id"))?;
    let receipt = Receipt {
        sender: sender.into(),
        harness: harness.into(),
        session: session.into(),
    };
    save(
        &receipt_path(name, mid)?,
        &serde_json::to_vec(&receipt).unwrap(),
    )?;
    let mut result = result;
    result["peer"] = json!(name);
    result["idempotency_key"] = json!(key);
    result["pickup"] = json!("remote inbox polling or hook; replies via peer status");
    Ok(result)
}

pub fn status(name: &str, message_id: &str) -> Result<Value> {
    assert_alias_unambiguous(name)?;
    if crate::relay::configured(name) {
        return crate::relay::status(name, message_id);
    }
    let receipt: Receipt =
        serde_json::from_slice(&fs::read(receipt_path(name, message_id)?).map_err(io_error)?)
            .map_err(|e| bad(format!("invalid outgoing receipt: {e}")))?;
    let mut result = rpc(
        name,
        &Request::Status {
            message_id: message_id.into(),
        },
    )?;
    if let Some(reply) = result.get("reply").and_then(Value::as_object) {
        let id = reply
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("reply missing id"))?;
        valid(id)?;
        let hops = reply
            .get("hops")
            .and_then(Value::as_i64)
            .ok_or_else(|| bad("reply missing hops"))?;
        if !(1..=inbox::MAX_HOPS).contains(&hops)
            || reply.get("reply_to") != Some(&json!(message_id))
        {
            return Err(bad("invalid peer reply correlation or hop count"));
        }
        let _guard = delivery_lock()?;
        let imported = inbox::read(&receipt.sender).into_iter().find(|m| {
            m.get("peer_reply_id") == Some(&json!(id)) && m.get("peer") == Some(&json!(name))
        });
        let message = if let Some(imported) = imported {
            imported
        } else {
            let mut extra = Map::new();
            extra.insert("peer_reply_id".into(), json!(id));
            extra.insert("peer".into(), json!(name));
            inbox::post(
                &receipt.sender,
                reply
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad("reply missing text"))?,
                &format!("{name}/{}/{}", receipt.harness, receipt.session),
                "reply",
                message_id,
                hops,
                &receipt.harness,
                "",
                Some(extra),
            )?
        };
        result["local_reply_id"] = message["id"].clone();
    }
    Ok(result)
}

fn remove_ssh_locked(name: &str) -> Result<Value> {
    rpc(name, &Request::Revoke)?;
    forget_ssh_locked(name)?;
    Ok(json!({"removed": name, "remote_key_revoked": true}))
}

fn forget_ssh_locked(name: &str) -> Result<()> {
    match fs::remove_file(known_hosts(name)?) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(io_error(e)),
    }
    fs::remove_file(config_path(name)?).map_err(io_error)
}

pub(crate) fn forget_device_locked(device_id: &str) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for (name, peer) in peers()? {
        if peer.device_id == device_id {
            forget_ssh_locked(&name)?;
            removed.push(name);
        }
    }
    Ok(removed)
}

/// Removing a duplicate alias is fail-closed unless a transport is named.
/// SSH removal keeps its remote key-revocation step; Convex removal is local
/// trust only, with permanent backend revocation remaining an explicit action.
pub fn remove_scoped(name: &str, transport: Option<&str>) -> Result<Value> {
    valid(name)?;
    let _alias_guard = alias_lock()?;
    let ssh = config_path(name)?.exists();
    let convex = crate::relay::configured(name);
    remove_alias_binding(
        ssh,
        convex,
        transport,
        || remove_ssh_locked(name),
        || crate::relay::remove_locked(name),
    )
}

fn remove_alias_binding<T>(
    ssh: bool,
    convex: bool,
    transport: Option<&str>,
    remove_ssh: impl FnOnce() -> Result<T>,
    remove_convex: impl FnOnce() -> Result<T>,
) -> Result<T> {
    match transport {
        None if ssh && convex => Err(bad(
            "alias is configured for both SSH and Convex; remove one binding explicitly before use",
        )),
        None if convex => remove_convex(),
        None if ssh => remove_ssh(),
        None => Err(bad("unknown peer; use everett peer add")),
        Some("ssh") if ssh => remove_ssh(),
        Some("convex") if convex => remove_convex(),
        Some("ssh") => Err(bad("alias has no SSH binding")),
        Some("convex") => Err(bad("alias has no Convex binding")),
        Some(_) => Err(bad("transport must be ssh or convex")),
    }
}

pub fn remove(name: &str) -> Result<Value> {
    remove_scoped(name, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_alias_lock_serializes_transport_claims() {
        let dir = tempfile::tempdir().unwrap();
        let held = alias_lock_at(dir.path()).unwrap();
        let path = dir.path().to_path_buf();
        let (sent, received) = std::sync::mpsc::channel();
        let contender = std::thread::spawn(move || {
            let _guard = alias_lock_at(&path).unwrap();
            sent.send(()).unwrap();
        });
        assert!(received
            .recv_timeout(std::time::Duration::from_millis(25))
            .is_err());
        drop(held);
        received
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        contender.join().unwrap();
    }

    #[test]
    fn shared_alias_claim_allows_exactly_one_competing_transport() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().to_path_buf();
        let claims = std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeSet::new()));
        let start = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut contenders = Vec::new();
        for (transport, other) in [("ssh", "convex"), ("convex", "ssh")] {
            let shared = shared.clone();
            let claims = claims.clone();
            let start = start.clone();
            contenders.push(std::thread::spawn(move || {
                start.wait();
                claim_alias_at(
                    &shared,
                    "shared-name",
                    other,
                    || Ok(claims.lock().unwrap().contains(other)),
                    || {
                        claims.lock().unwrap().insert(transport.to_string());
                        Ok(transport.to_string())
                    },
                )
            }));
        }
        let results: Vec<_> = contenders.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
        assert_eq!(claims.lock().unwrap().len(), 1);
        assert!(results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .all(|e| e.message.contains("alias already selects")));
    }

    #[test]
    fn duplicate_alias_removal_is_fail_closed_but_scope_repairs_one_binding() {
        let bindings = std::sync::Arc::new(std::sync::Mutex::new(
            ["ssh", "convex"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
        ));
        let ssh = || {
            let bindings = bindings.clone();
            move || {
                bindings.lock().unwrap().remove("ssh");
                Ok("ssh")
            }
        };
        let convex = || {
            let bindings = bindings.clone();
            move || {
                bindings.lock().unwrap().remove("convex");
                Ok("convex")
            }
        };
        assert!(remove_alias_binding(true, true, None, ssh(), convex()).is_err());
        assert_eq!(bindings.lock().unwrap().len(), 2);
        assert_eq!(
            remove_alias_binding(true, true, Some("ssh"), ssh(), convex()).unwrap(),
            "ssh"
        );
        assert_eq!(
            bindings.lock().unwrap().iter().copied().collect::<Vec<_>>(),
            ["convex"]
        );
    }
}
