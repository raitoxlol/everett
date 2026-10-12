use super::*;
use crypto::Scope;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub owner: String,
    pub org: String,
    pub agent_id: String,
    pub device: Device,
}

impl Agent {
    pub fn fingerprint(&self) -> String {
        format!(
            "SHA256:{:x}",
            Sha256::digest(json!([self.org, self.agent_id, self.device]).to_string())
        )
    }

    fn verify(&self) -> Result<()> {
        self.device.verify()?;
        if self.device.owner != self.owner {
            return Err(bad("agent account does not match its device"));
        }
        for id in [&self.org, &self.agent_id] {
            if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad("invalid issuer-derived agent identity"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Identity {
    owner: String,
    org: String,
    agent_id: Option<String>,
    credential_id: Option<String>,
    admin: bool,
    #[serde(skip)]
    deployment: String,
}

impl Identity {
    fn agent(&self, keys: &Keys) -> Result<Agent> {
        let a = Agent {
            owner: self.owner.clone(),
            org: self.org.clone(),
            agent_id: self
                .agent_id
                .clone()
                .ok_or_else(|| bad("OIDC provider must issue agent_id and jti claims"))?,
            device: keys.device.clone(),
        };
        a.verify()?;
        Ok(a)
    }

    fn root(&self) -> PathBuf {
        root().join("agents").join(format!(
            "{:x}",
            Sha256::digest(
                json!([self.owner, self.org, self.agent_id, self.deployment]).to_string()
            )
        ))
    }
}

#[derive(Serialize, Deserialize)]
struct Pin {
    agent: Agent,
    deployment: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Attempt {
    agent: Agent,
    envelope: Envelope,
    digest: String,
    posted: bool,
    state: State,
    session: String,
    harness: String,
}

#[derive(Deserialize)]
struct Incoming {
    envelope: Envelope,
    external: bool,
    trust: String,
}

async fn query(client: &mut ConvexClient, name: &str, value: Value) -> Result<Value> {
    result(
        tokio::time::timeout(DEADLINE, client.query(name, args(value)?))
            .await
            .map_err(|_| bad("Convex query timed out"))?
            .map_err(|_| bad("Convex query failed"))?,
    )
}

async fn identity(client: &mut ConvexClient, owner: &str) -> Result<Identity> {
    let mut p: Identity =
        serde_json::from_value(query(client, "agents:identity", json!({})).await?).map_err(bad)?;
    if p.owner != owner {
        return Err(bad("signed-in identity changed"));
    }
    p.deployment = deployment()?;
    Ok(p)
}

fn initialize(p: &Identity) -> Result<()> {
    for dir in [
        root().join("agents"),
        p.root(),
        p.root().join("pins"),
        p.root().join("outgoing"),
    ] {
        peer::private_dir(&dir)?;
    }
    Ok(())
}

fn pinned(p: &Identity, id: &str) -> Result<Agent> {
    valid(id)?;
    let pin: Pin = read(&p.root().join("pins").join(format!("{id}.json")))?;
    pin.agent.verify()?;
    if pin.agent.agent_id != id || pin.deployment != deployment()? {
        return Err(bad("agent pin identity or deployment mismatch"));
    }
    Ok(pin.agent)
}

fn agent_keys(p: &Identity) -> Result<Keys> {
    if p.agent_id.is_none() {
        return Err(bad("issuer agent_id required"));
    }
    initialize(p)?;
    let _guard = peer::lock(&p.root().join("keys.lock"))?;
    let path = p.root().join("keys.json");
    let (id, signing, encryption): (String, String, String) = if path.exists() {
        read(&path)?
    } else {
        let mut signing = [0u8; 32];
        let mut encryption = [0u8; 32];
        OsRng.fill_bytes(&mut signing);
        OsRng.fill_bytes(&mut encryption);
        let keys = (
            uuid::Uuid::new_v4().to_string(),
            hex::encode(signing),
            hex::encode(encryption),
        );
        write(&path, &keys)?;
        keys
    };
    Ok(Keys::new(
        p.owner.clone(),
        id,
        crypto::decode(&signing)?,
        crypto::decode(&encryption)?,
    ))
}

pub fn control(action: &str, id: &str, remote: &str, fingerprint: &str) -> Result<Value> {
    let (_, owner) = account()?;
    let action = action.to_string();
    let id = id.to_string();
    let remote = remote.to_string();
    let fingerprint = fingerprint.to_string();
    runtime(async move {
        let mut c = client(&owner).await?;
        let p = identity(&mut c, &owner).await?;
        initialize(&p)?;
        match action.as_str() {
            "identity" => Ok(serde_json::to_value(p).map_err(bad)?),
            "register" => {
                let keys = agent_keys(&p)?;
                let a = p.agent(&keys)?;
                let response =
                    mutation(&mut c, "agents:register", json!({"device": keys.device})).await?;
                Ok(json!({"identity":response,"fingerprint":a.fingerprint()}))
            }
            "directory" => {
                let mut rows = query(&mut c, "agents:directory", json!({})).await?;
                for row in rows
                    .as_array_mut()
                    .ok_or_else(|| bad("invalid agent directory"))?
                {
                    let a: Agent = serde_json::from_value(row.clone()).map_err(bad)?;
                    a.verify()?;
                    row["fingerprint"] = json!(a.fingerprint());
                }
                Ok(rows)
            }
            "grants" => query(&mut c, "agents:grants", json!({})).await,
            "memberships" => query(&mut c, "agents:memberships", json!({})).await,
            "credentials" => query(&mut c, "agents:credentials", json!({})).await,
            "revoke-membership" => {
                mutation(
                    &mut c,
                    "agents:revokeMembership",
                    json!({"membershipId":id}),
                )
                .await
            }
            "propose" => {
                mutation(
                    &mut c,
                    "agents:propose",
                    json!({"localAgent":id,"remoteAgent":remote}),
                )
                .await
            }
            "approve" | "revoke-grant" => {
                mutation(
                    &mut c,
                    "agents:decide",
                    json!({"grantId":id,"approve":action == "approve"}),
                )
                .await
            }
            "revoke-agent" => mutation(&mut c, "agents:revoke", json!({"agentId":id})).await,
            "revoke-credential" => {
                mutation(
                    &mut c,
                    "agents:revokeCredential",
                    json!({"credentialId":id}),
                )
                .await
            }
            "pair" => {
                let keys = agent_keys(&p)?;
                p.agent(&keys)?;
                valid(&id)?;
                let mut candidates: Vec<Agent> =
                    serde_json::from_value(query(&mut c, "agents:directory", json!({})).await?)
                        .map_err(bad)?;
                let grants = query(&mut c, "agents:grants", json!({})).await?;
                for grant in grants.as_array().ok_or_else(|| bad("invalid grants"))? {
                    if grant["active"] == true && !grant["remote"].is_null() {
                        candidates
                            .push(serde_json::from_value(grant["remote"].clone()).map_err(bad)?);
                    }
                }
                let a = candidates
                    .into_iter()
                    .find(|a| a.agent_id == id)
                    .ok_or_else(|| {
                        bad("agent is not in your org or an active named-agent grant")
                    })?;
                a.verify()?;
                if a.fingerprint() != fingerprint {
                    return Err(bad(
                        "agent fingerprint mismatch; confirm identity, org and keys out of band",
                    ));
                }
                if p.agent_id.as_deref() == Some(&id) {
                    return Err(bad("cannot pair yourself"));
                }
                let path = p.root().join("pins").join(format!("{id}.json"));
                let _guard = peer::lock(&p.root().join("pins.lock"))?;
                if path.exists() {
                    let old: Pin = read(&path)?;
                    if old.agent.fingerprint() != fingerprint || old.deployment != deployment()? {
                        return Err(bad(
                            "pin changed; explicitly unpair before confirming new keys",
                        ));
                    }
                }
                write(
                    &path,
                    &Pin {
                        agent: a,
                        deployment: deployment()?,
                    },
                )?;
                Ok(json!({"paired":id,"fingerprint":fingerprint}))
            }
            "unpair" => {
                valid(&id)?;
                fs::remove_file(p.root().join("pins").join(format!("{id}.json"))).map_err(bad)?;
                Ok(json!({"unpaired":id}))
            }
            _ => Err(bad("unknown agent relay action")),
        }
    })
}

fn scope(from: &Agent, to: &Agent) -> Scope {
    Scope {
        from_owner: from.owner.clone(),
        to_owner: to.owner.clone(),
        from_org: from.org.clone(),
        to_org: to.org.clone(),
        from_agent: from.agent_id.clone(),
        to_agent: to.agent_id.clone(),
    }
}

fn prepare(
    p: &Identity,
    keys: &Keys,
    to: &Agent,
    key: &str,
    payload: &Payload,
) -> Result<(PathBuf, Attempt)> {
    valid(key)?;
    let from = p.agent(keys)?;
    payload.validate()?;
    let digest = format!(
        "{:x}",
        Sha256::digest(json!([to.agent_id, payload]).to_string())
    );
    let path = p
        .root()
        .join("outgoing")
        .join(format!("{:x}.json", Sha256::digest(key)));
    let _guard = peer::lock(&p.root().join("outgoing.lock"))?;
    if path.exists() {
        let old: Attempt = read(&path)?;
        if old.digest != digest || old.agent.fingerprint() != to.fingerprint() {
            return Err(bad(
                "idempotency key already used for a different agent request",
            ));
        }
        return Ok((path, old));
    }
    let (session, harness) = match payload {
        Payload::Inbox {
            from, from_harness, ..
        } => (from.clone(), from_harness.clone()),
        _ => (String::new(), String::new()),
    };
    let out = Attempt {
        agent: to.clone(),
        envelope: keys.seal_agent(&to.device, key, payload, milliseconds(), scope(&from, to))?,
        digest,
        posted: false,
        state: State::Queued,
        session,
        harness,
    };
    write(&path, &out)?;
    Ok((path, out))
}

async fn post(c: &mut ConvexClient, p: &Identity, path: &Path, out: &Attempt) -> Result<Value> {
    let pin = pinned(p, &out.agent.agent_id)?;
    if pin.fingerprint() != out.agent.fingerprint() {
        return Err(bad("agent pin changed"));
    }
    let response = mutation(c, "agents:enqueue", json!({"envelope":out.envelope})).await?;
    let _guard = peer::lock(&p.root().join("outgoing.lock"))?;
    let mut current: Attempt = read(path)?;
    current.posted = true;
    write(path, &current)?;
    Ok(response)
}

pub fn send(
    to: &str,
    session: &str,
    harness: &str,
    text: &str,
    key: &str,
    caller: Option<&str>,
) -> Result<Value> {
    valid(to)?;
    if !inbox::valid_id(session) {
        return Err(bad("invalid recipient session"));
    }
    valid(harness)?;
    let who = send::sender_info(caller);
    let hops = send::current_hops()
        .max(inbox::session_hops(
            who["sender"].as_str().unwrap_or(inbox::HUMAN),
        ))
        .max(0)
        + 1;
    let payload = Payload::Inbox {
        session: session.into(),
        harness: harness.into(),
        text: text.trim().into(),
        from: who["sender"].as_str().unwrap_or(inbox::HUMAN).into(),
        from_harness: who["from_harness"].as_str().unwrap_or("").into(),
        from_card: who["from_card"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(160)
            .collect(),
        reply_to: String::new(),
        hops,
    };
    submit(to, key, payload)
}

fn submit(to: &str, key: &str, payload: Payload) -> Result<Value> {
    let (_, owner) = account()?;
    let to = to.to_string();
    let key = key.to_string();
    runtime(async move {
        let mut c = client(&owner).await?;
        let p = identity(&mut c, &owner).await?;
        initialize(&p)?;
        let keys = agent_keys(&p)?;
        let recipient = pinned(&p, &to)?;
        let (path, out) = prepare(&p, &keys, &recipient, &key, &payload)?;
        if out.envelope.expires_at <= milliseconds() {
            return Err(bad(
                "request expired; use a new key only for an intentionally new send",
            ));
        }
        post(&mut c, &p, &path, &out).await?;
        let current: Attempt = read(&path)?;
        Ok(
            json!({"message_id":out.envelope.message_id,"mode":"agent-relay","state":current.state,"pickup":"encrypted inbox only; no automatic wake-up"}),
        )
    })
}

pub fn reply(
    original: &Map<String, Value>,
    text: &str,
    who: &Map<String, Value>,
    hops: i64,
) -> Result<Map<String, Value>> {
    let id = original["id"]
        .as_str()
        .ok_or_else(|| bad("missing message ID"))?;
    let to = original["agent_sender"]
        .as_str()
        .ok_or_else(|| bad("missing agent sender"))?;
    let (_, owner) = account()?;
    if original["agent_recipient_owner"] != owner {
        return Err(bad("message belongs to another agent account"));
    }
    let recipient = original["agent_recipient"].clone();
    runtime(async move {
        let mut c = client(&owner).await?;
        let p = identity(&mut c, &owner).await?;
        if recipient != p.agent_id.unwrap_or_default() {
            return Err(bad("message belongs to another named agent"));
        }
        Ok(())
    })?;
    let payload = Payload::Inbox {
        session: original["agent_sender_session"]
            .as_str()
            .ok_or_else(|| bad("missing reply session"))?
            .into(),
        harness: original["from_harness"].as_str().unwrap_or("").into(),
        text: text.trim().into(),
        from: who["sender"].as_str().unwrap_or(inbox::HUMAN).into(),
        from_harness: who["from_harness"].as_str().unwrap_or("").into(),
        from_card: who["from_card"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(160)
            .collect(),
        reply_to: id.into(),
        hops,
    };
    let result = submit(to, &format!("reply-{id}"), payload)?;
    serde_json::from_value(result).map_err(bad)
}

fn find(p: &Identity, mid: &str) -> Result<(PathBuf, Attempt)> {
    valid(mid)?;
    for path in crate::paths::glob(&p.root().join("outgoing"), "*.json") {
        let out: Attempt = read(&path)?;
        if out.envelope.message_id == mid {
            return Ok((path, out));
        }
    }
    Err(bad("no outgoing message for this agent"))
}

pub fn status(mid: &str) -> Result<Value> {
    valid(mid)?;
    let (_, owner) = account()?;
    let mid = mid.to_string();
    runtime(async move {
        let mut c = client(&owner).await?;
        let p = identity(&mut c, &owner).await?;
        let response = query(&mut c, "agents:status", json!({"messageId":mid})).await?;
        let (_, out) = find(&p, &mid)?;
        let state = if out.state != State::Queued {
            json!(out.state)
        } else if out.envelope.expires_at <= milliseconds() {
            json!("expired")
        } else {
            json!("queued")
        };
        Ok(
            json!({"message_id":mid,"state":state,"server_state":response["state"],"note":"accepted means durable local import, not agent pickup; delivered/replied require verified encrypted observations"}),
        )
    })
}

fn observe(
    p: &Identity,
    agent: &Agent,
    mid: &str,
    state: State,
    session: Option<(&str, &str)>,
) -> Result<()> {
    let _guard = peer::lock(&p.root().join("outgoing.lock"))?;
    let (path, mut out) = find(p, mid)?;
    if out.agent.fingerprint() != agent.fingerprint()
        || session.is_some_and(|(s, h)| s != out.session || h != out.harness)
    {
        return Err(bad("agent receipt or reply does not match the request"));
    }
    if out.state != State::Replied {
        out.state = state;
    }
    write(&path, &out)
}

fn import(
    p: &Identity,
    keys: &Keys,
    incoming: &Incoming,
    session: &str,
    harness: &str,
) -> Result<()> {
    let e = &incoming.envelope;
    let s = e
        .scope
        .as_ref()
        .ok_or_else(|| bad("agent envelope lacks scope"))?;
    let local = p.agent(keys)?;
    let sender = pinned(p, &s.from_agent)?;
    if *s != scope(&sender, &local)
        || incoming.external != (sender.org != local.org)
        || incoming.trust
            != if incoming.external {
                "external-untrusted"
            } else {
                "same-org"
            }
    {
        return Err(bad("agent identity, org scope or trust label mismatch"));
    }
    let payload = keys.open(&sender.device, e, milliseconds())?;
    match payload {
        Payload::Delivered { message_id } => {
            observe(p, &sender, &message_id, State::Delivered, None)
        }
        Payload::Inbox {
            session: target,
            harness: target_harness,
            text,
            from,
            from_harness,
            from_card,
            reply_to,
            hops,
        } => {
            if target != session || target_harness != harness {
                return Err(bad(
                    "message is not addressed to this named agent's bound local session",
                ));
            }
            if !reply_to.is_empty() {
                let (_, out) = find(p, &reply_to)?;
                if out.agent.fingerprint() != sender.fingerprint()
                    || out.session != target
                    || out.harness != target_harness
                {
                    return Err(bad("reply does not match original request"));
                }
            }
            let digest = format!("{:x}", Sha256::digest(e.signed_bytes()));
            if let Some(old) = inbox::find(&e.message_id) {
                if old["agent_digest"] != digest || old["to"] != target {
                    return Err(bad(
                        "duplicate agent message has different ciphertext or recipient",
                    ));
                }
            } else {
                let extra = serde_json::from_value(json!({"id":e.message_id,"agent_sender":sender.agent_id,"agent_sender_session":from,
                    "agent_recipient":local.agent_id,"agent_recipient_owner":local.owner,"agent_org":sender.org,
                    "agent_digest":digest,"external":incoming.external,"trust":incoming.trust})).map_err(bad)?;
                inbox::post(
                    &target,
                    &text,
                    &sender.agent_id,
                    if reply_to.is_empty() {
                        "message"
                    } else {
                        "reply"
                    },
                    &reply_to,
                    hops,
                    &from_harness,
                    &from_card,
                    Some(extra),
                )?;
                fs::OpenOptions::new()
                    .write(true)
                    .open(inbox::path(&target)?)
                    .map_err(bad)?
                    .sync_all()
                    .map_err(bad)?;
            }
            if !reply_to.is_empty() {
                observe(
                    p,
                    &sender,
                    &reply_to,
                    State::Replied,
                    Some((&target, &target_harness)),
                )?;
            }
            Ok(())
        }
    }
}

async fn reconcile(c: &mut ConvexClient, p: &Identity, keys: &Keys, session: &str) -> Result<()> {
    let done = inbox::done_ids(session);
    for message in inbox::read(session) {
        let Some(mid) = message["id"].as_str() else {
            continue;
        };
        if message["agent_recipient"] != p.agent_id.clone().unwrap_or_default()
            || message["agent_recipient_owner"] != p.owner
            || !done.contains(mid)
        {
            continue;
        }
        let to = message["agent_sender"]
            .as_str()
            .ok_or_else(|| bad("missing sender agent"))?;
        let recipient = pinned(p, to)?;
        let (path, out) = prepare(
            p,
            keys,
            &recipient,
            &format!("delivered-{mid}"),
            &Payload::Delivered {
                message_id: mid.into(),
            },
        )?;
        if !out.posted && out.envelope.expires_at > milliseconds() {
            post(c, p, &path, &out).await?;
        }
    }
    for path in crate::paths::glob(&p.root().join("outgoing"), "*.json") {
        let out: Attempt = read(&path)?;
        if !out.posted && out.envelope.expires_at > milliseconds() {
            post(c, p, &path, &out).await?;
        }
    }
    Ok(())
}

pub fn run(session: &str, harness: &str) -> Result<Value> {
    if !inbox::valid_id(session) {
        return Err(bad("invalid bound agent session"));
    }
    valid(harness)?;
    let (_, owner) = account()?;
    tokio::runtime::Runtime::new().map_err(bad)?.block_on(async {
        let mut c = client(&owner).await?;
        let p = identity(&mut c, &owner).await?;
        initialize(&p)?;
        let keys = agent_keys(&p)?;
        p.agent(&keys)?;
        let _guard = peer::lock(&p.root().join("daemon.lock"))?;
        mutation(&mut c, "agents:register", json!({"device":keys.device})).await?;
        let mut subscription = c.subscribe("agents:pending", args(json!({}))?).await.map_err(|_| bad("agent subscription failed"))?;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut pending = Vec::<Incoming>::new();
        let mut rejected = std::collections::HashSet::new();
        eprintln!("everett relay: scoped agent subscribed; external messages are untrusted; no session wake-up");
        loop {
            tokio::select! {
                update = subscription.next() => {
                    pending = serde_json::from_value(result(update.ok_or_else(|| bad("agent subscription ended"))?)?).map_err(bad)?;
                    tick.reset_immediately();
                }
                _ = tick.tick() => {
                    let current = identity(&mut c, &owner).await?;
                    if current.org != p.org || current.agent_id != p.agent_id || current.credential_id != p.credential_id {
                        return Err(bad("OIDC agent identity or credential changed; restart the relay"));
                    }
                    for incoming in &pending {
                        // Reauthorize cached subscription rows before any local plaintext write.
                        let fresh: Vec<Incoming> = serde_json::from_value(query(&mut c, "agents:pending", json!({})).await?).map_err(bad)?;
                        if !fresh.iter().any(|row| row.envelope == incoming.envelope) { continue; }
                        if let Err(error) = import(&p, &keys, incoming, session, harness) {
                            if rejected.insert(incoming.envelope.message_id.clone()) { eprintln!("everett relay: {}", error.message); }
                            continue;
                        }
                        let s = incoming.envelope.scope.as_ref().unwrap();
                        let at = milliseconds();
                        let signature = keys.sign(json!(["everett-agent-accept-v2", p.org, p.agent_id, s.from_org, incoming.envelope.message_id, at]).to_string().as_bytes());
                        mutation(&mut c, "agents:accept", json!({"fromOrg":s.from_org,"messageId":incoming.envelope.message_id,"at":at,"signature":signature})).await?;
                    }
                    if let Err(error) = reconcile(&mut c, &p, &keys, session).await { eprintln!("everett relay: {}", error.message); }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests;
