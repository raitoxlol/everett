use super::crypto::{verify, Device, Keys, ThreadCipher};
use super::{bad, valid};
use crate::error::Result;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const MAX_TEXT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadRef {
    pub creator_owner: String,
    pub thread_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadMember {
    pub address: String,
    pub owner: String,
    pub agent_id: String,
    pub harness: String,
    pub session: String,
    pub fingerprint: String,
    pub grant_id: String,
    pub device: Device,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadContext {
    pub creator_owner: String,
    pub thread_id: String,
    pub message_id: String,
    pub idempotency_key: String,
    pub author_owner: String,
    pub author_device_id: String,
    pub author_fingerprint: String,
    pub commitment: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadCopy {
    pub recipient: ThreadMember,
    pub ephemeral_key: String,
    pub nonce: String,
    pub ciphertext: String,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadPost {
    pub context: ThreadContext,
    pub copies: [ThreadCopy; 2],
    pub pair_signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OutboxRecord {
    thread_ref: ThreadRef,
    idempotency_key: String,
    post: ThreadPost,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredThreadMessage {
    pub seq: u64,
    pub context: ThreadContext,
    pub copy: ThreadCopy,
    pub pair_signature: String,
    pub created_at: i64,
}

fn context_fields(c: &ThreadContext) -> Vec<String> {
    vec![
        c.creator_owner.clone(),
        c.thread_id.clone(),
        c.message_id.clone(),
        c.idempotency_key.clone(),
        c.author_owner.clone(),
        c.author_device_id.clone(),
        c.author_fingerprint.clone(),
        c.commitment.clone(),
    ]
}

fn member_fields(m: &ThreadMember) -> Vec<String> {
    vec![
        m.address.clone(),
        m.owner.clone(),
        m.agent_id.clone(),
        m.harness.clone(),
        m.session.clone(),
        m.fingerprint.clone(),
        m.grant_id.clone(),
    ]
}

pub fn aad_bytes(context: &ThreadContext, recipient: &ThreadMember) -> Vec<u8> {
    let mut fields = vec![json!("everett-thread-aad-v1")];
    fields.extend(context_fields(context).into_iter().map(|v| json!(v)));
    fields.extend(member_fields(recipient).into_iter().map(|v| json!(v)));
    serde_json::Value::Array(fields).to_string().into_bytes()
}

pub fn copy_bytes(context: &ThreadContext, copy: &ThreadCopy) -> Vec<u8> {
    let mut fields = vec![json!("everett-thread-copy-v1")];
    fields.extend(context_fields(context).into_iter().map(|v| json!(v)));
    fields.extend(member_fields(&copy.recipient).into_iter().map(|v| json!(v)));
    fields.extend([
        json!(copy.ephemeral_key),
        json!(copy.nonce),
        json!(copy.ciphertext),
    ]);
    serde_json::Value::Array(fields).to_string().into_bytes()
}

pub fn pair_bytes(context: &ThreadContext, copies: &[ThreadCopy; 2]) -> Vec<u8> {
    let copies = copies
        .iter()
        .map(|copy| {
            let mut fields = member_fields(&copy.recipient)
                .into_iter()
                .map(|v| json!(v))
                .collect::<Vec<_>>();
            fields.extend([
                json!(copy.ephemeral_key),
                json!(copy.nonce),
                json!(copy.ciphertext),
                json!(copy.signature),
            ]);
            serde_json::Value::Array(fields)
        })
        .collect::<Vec<_>>();
    let mut fields = vec![json!("everett-thread-pair-v1")];
    fields.extend(context_fields(context).into_iter().map(|v| json!(v)));
    fields.push(json!(copies));
    serde_json::Value::Array(fields).to_string().into_bytes()
}

fn canonical_payload(text: &str) -> Vec<u8> {
    let text = text.as_bytes();
    let mut payload = b"everett-thread-payload-v1\0".to_vec();
    payload.extend_from_slice(&(text.len() as u32).to_be_bytes());
    payload.extend_from_slice(text);
    payload
}

fn payload_record(secret: &[u8; 32], text: &str) -> Vec<u8> {
    let mut record = b"everett-thread-record-v1\0".to_vec();
    record.extend_from_slice(secret);
    record.extend_from_slice(&canonical_payload(text));
    record
}

fn decode_payload_record(record: &[u8]) -> Result<([u8; 32], String)> {
    const RECORD: &[u8] = b"everett-thread-record-v1\0";
    const PAYLOAD: &[u8] = b"everett-thread-payload-v1\0";
    if !record.starts_with(RECORD) || record.len() < RECORD.len() + 32 + PAYLOAD.len() + 4 {
        return Err(bad("invalid strict thread payload record"));
    }
    let secret_start = RECORD.len();
    let secret: [u8; 32] = record[secret_start..secret_start + 32]
        .try_into()
        .map_err(|_| bad("invalid thread blinding secret"))?;
    let payload = &record[secret_start + 32..];
    if !payload.starts_with(PAYLOAD) {
        return Err(bad("invalid strict thread payload version"));
    }
    let length_offset = PAYLOAD.len();
    let length = u32::from_be_bytes(
        payload[length_offset..length_offset + 4]
            .try_into()
            .map_err(bad)?,
    ) as usize;
    let text = payload
        .get(length_offset + 4..)
        .ok_or_else(|| bad("invalid strict thread payload length"))?;
    if length != text.len() || length == 0 || length > MAX_TEXT_BYTES {
        return Err(bad("invalid strict thread text payload"));
    }
    let text = String::from_utf8(text.to_vec()).map_err(bad)?;
    Ok((secret, text))
}

fn commitment(secret: &[u8; 32], text: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"everett-thread-content-v1\0");
    hash.update(secret);
    hash.update(canonical_payload(text));
    hex::encode(hash.finalize())
}

fn validate_ref(thread_ref: &ThreadRef) -> Result<()> {
    if thread_ref.creator_owner.is_empty() || thread_ref.creator_owner.len() > 4096 {
        return Err(bad("invalid thread creator owner"));
    }
    uuid::Uuid::parse_str(&thread_ref.thread_id).map_err(bad)?;
    Ok(())
}

fn validate_members(thread_ref: &ThreadRef, members: &[ThreadMember; 2]) -> Result<()> {
    if members[0].owner == members[1].owner || members[0].owner != thread_ref.creator_owner {
        return Err(bad(
            "thread must contain two distinct owners and its creator",
        ));
    }
    for member in members {
        valid(&member.agent_id)?;
        valid(&member.harness)?;
        valid(&member.session)?;
        valid(&member.grant_id)?;
        if !super::inbox_address(&member.address)
            || member.owner.is_empty()
            || member.owner.len() > 4096
        {
            return Err(bad("invalid thread member"));
        }
        member.device.verify()?;
        if member.device.owner != member.owner
            || member.device.device_id != member.agent_id
            || member.device.fingerprint() != member.fingerprint
        {
            return Err(bad("thread member device binding changed"));
        }
    }
    Ok(())
}

pub fn create_post(
    keys: &Keys,
    thread_ref: &ThreadRef,
    members: &[ThreadMember; 2],
    idempotency_key: &str,
    text: &str,
) -> Result<ThreadPost> {
    validate_ref(thread_ref)?;
    validate_members(thread_ref, members)?;
    valid(idempotency_key)?;
    if text.is_empty() || text.len() > MAX_TEXT_BYTES {
        return Err(bad(
            "thread text must be non-empty and at most 16 KiB UTF-8",
        ));
    }
    let author = members
        .iter()
        .find(|member| {
            member.owner == keys.device.owner && member.agent_id == keys.device.device_id
        })
        .ok_or_else(|| bad("authenticated device is not a thread member"))?;
    if author.fingerprint != keys.device.fingerprint() {
        return Err(bad("thread author device key changed"));
    }
    let mut secret = [0u8; 32];
    OsRng.fill_bytes(&mut secret);
    let context = ThreadContext {
        creator_owner: thread_ref.creator_owner.clone(),
        thread_id: thread_ref.thread_id.clone(),
        message_id: uuid::Uuid::new_v4().to_string(),
        idempotency_key: idempotency_key.to_string(),
        author_owner: author.owner.clone(),
        author_device_id: author.agent_id.clone(),
        author_fingerprint: author.fingerprint.clone(),
        commitment: commitment(&secret, text),
    };
    let body = payload_record(&secret, text);
    let mut copies = Vec::with_capacity(2);
    for recipient in members {
        let aad = aad_bytes(&context, recipient);
        let encrypted = keys.seal_thread_bytes(&recipient.device, &aad, &body)?;
        let mut copy = ThreadCopy {
            recipient: recipient.clone(),
            ephemeral_key: encrypted.ephemeral_key,
            nonce: encrypted.nonce,
            ciphertext: encrypted.ciphertext,
            signature: String::new(),
        };
        copy.signature = keys.sign(&copy_bytes(&context, &copy));
        copies.push(copy);
    }
    let copies: [ThreadCopy; 2] = copies
        .try_into()
        .map_err(|_| bad("thread pair must contain two copies"))?;
    let pair_signature = keys.sign(&pair_bytes(&context, &copies));
    Ok(ThreadPost {
        context,
        copies,
        pair_signature,
    })
}

pub fn verify_post(author: &Device, post: &ThreadPost) -> Result<()> {
    author.verify()?;
    if post.context.author_owner != author.owner
        || post.context.author_device_id != author.device_id
        || post.context.author_fingerprint != author.fingerprint()
    {
        return Err(bad("thread author context mismatch"));
    }
    for copy in &post.copies {
        verify(
            &author.signing_key,
            &copy.signature,
            &copy_bytes(&post.context, copy),
        )?;
    }
    verify(
        &author.signing_key,
        &post.pair_signature,
        &pair_bytes(&post.context, &post.copies),
    )
}

pub fn persist_outgoing(
    path: &Path,
    keys: &Keys,
    thread_ref: &ThreadRef,
    members: &[ThreadMember; 2],
    idempotency_key: &str,
    text: &str,
) -> Result<ThreadPost> {
    let parent = path
        .parent()
        .ok_or_else(|| bad("missing thread outbox directory"))?;
    crate::storage::private_dir(parent)?;
    let lock = crate::storage::lock(&path.with_extension("lock"))?;
    let post = if path.exists() {
        if std::fs::metadata(path).map_err(bad)?.len() > 128_000 {
            return Err(bad("thread retry record exceeds size limit"));
        }
        let record: OutboxRecord =
            serde_json::from_slice(&std::fs::read(path).map_err(bad)?).map_err(bad)?;
        if record.thread_ref != *thread_ref || record.idempotency_key != idempotency_key {
            return Err(bad("thread retry record binding mismatch"));
        }
        verify_post(&keys.device, &record.post)?;
        let recipient = members
            .iter()
            .find(|member| {
                member.owner == keys.device.owner && member.agent_id == keys.device.device_id
            })
            .ok_or_else(|| bad("authenticated device is not a thread member"))?;
        let copy = record
            .post
            .copies
            .iter()
            .find(|copy| &copy.recipient == recipient)
            .ok_or_else(|| bad("thread retry record has no author copy"))?;
        let message = StoredThreadMessage {
            seq: 0,
            context: record.post.context.clone(),
            copy: copy.clone(),
            pair_signature: record.post.pair_signature.clone(),
            created_at: 0,
        };
        if open_copy(keys, thread_ref, members, &message)? != text {
            return Err(bad(
                "thread idempotency key was already used for different text",
            ));
        }
        record.post
    } else {
        let post = create_post(keys, thread_ref, members, idempotency_key, text)?;
        verify_post(&keys.device, &post)?;
        let record = OutboxRecord {
            thread_ref: thread_ref.clone(),
            idempotency_key: idempotency_key.to_string(),
            post: post.clone(),
        };
        let serialized = serde_json::to_vec(&record).map_err(bad)?;
        crate::storage::save(path, &serialized)?;
        std::fs::File::open(parent)
            .map_err(bad)?
            .sync_all()
            .map_err(bad)?;
        post
    };
    drop(lock);
    Ok(post)
}

pub fn open_copy(
    keys: &Keys,
    thread_ref: &ThreadRef,
    members: &[ThreadMember; 2],
    message: &StoredThreadMessage,
) -> Result<String> {
    validate_ref(thread_ref)?;
    validate_members(thread_ref, members)?;
    let context = &message.context;
    if context.creator_owner != thread_ref.creator_owner
        || context.thread_id != thread_ref.thread_id
    {
        return Err(bad("thread context mismatch"));
    }
    let author = members
        .iter()
        .find(|member| {
            member.owner == context.author_owner && member.agent_id == context.author_device_id
        })
        .ok_or_else(|| bad("thread author is not a member"))?;
    if author.fingerprint != context.author_fingerprint {
        return Err(bad("thread author fingerprint mismatch"));
    }
    verify(
        &author.device.signing_key,
        &message.copy.signature,
        &copy_bytes(context, &message.copy),
    )?;
    let recipient = members
        .iter()
        .find(|member| {
            member.owner == keys.device.owner && member.agent_id == keys.device.device_id
        })
        .ok_or_else(|| bad("authenticated device is not a thread member"))?;
    if &message.copy.recipient != recipient || recipient.fingerprint != keys.device.fingerprint() {
        return Err(bad("thread copy is not addressed to this device"));
    }
    let encrypted = ThreadCipher {
        ephemeral_key: message.copy.ephemeral_key.clone(),
        nonce: message.copy.nonce.clone(),
        ciphertext: message.copy.ciphertext.clone(),
    };
    let body = keys.open_thread_bytes(
        &recipient.device,
        &aad_bytes(context, recipient),
        &encrypted,
    )?;
    let (secret, text) = decode_payload_record(&body)?;
    if commitment(&secret, &text) != context.commitment {
        return Err(bad("thread content commitment mismatch"));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn member(owner: &str, id: &str, signing: u8, encryption: u8) -> (ThreadMember, Keys) {
        let keys = Keys::new(
            owner.to_string(),
            id.to_string(),
            [signing; 32],
            [encryption; 32],
        );
        let member = ThreadMember {
            address: format!("{id}@wmm.12345"),
            owner: owner.to_string(),
            agent_id: id.to_string(),
            harness: "claude".into(),
            session: format!("session-{id}"),
            fingerprint: keys.device.fingerprint(),
            grant_id: format!("grant-{id}"),
            device: keys.device.clone(),
        };
        (member, keys)
    }

    #[test]
    fn each_member_decrypts_the_same_committed_text() {
        let (a, ak) = member("owner-a", "agent-a", 1, 2);
        let (b, bk) = member("owner-b", "agent-b", 3, 4);
        let reference = ThreadRef {
            creator_owner: a.owner.clone(),
            thread_id: uuid::Uuid::new_v4().to_string(),
        };
        let members = [a, b];
        let post = create_post(&ak, &reference, &members, "retry-1", "shared text").unwrap();
        for (i, keys) in [&ak, &bk].into_iter().enumerate() {
            let message = StoredThreadMessage {
                seq: 1,
                context: post.context.clone(),
                copy: post.copies[i].clone(),
                pair_signature: post.pair_signature.clone(),
                created_at: 1,
            };
            assert_eq!(
                open_copy(keys, &reference, &members, &message).unwrap(),
                "shared text"
            );
        }
    }

    #[test]
    fn tampering_context_or_copy_recipient_fails() {
        let (a, ak) = member("owner-a", "agent-a", 1, 2);
        let (b, bk) = member("owner-b", "agent-b", 3, 4);
        let reference = ThreadRef {
            creator_owner: a.owner.clone(),
            thread_id: uuid::Uuid::new_v4().to_string(),
        };
        let members = [a, b];
        let post = create_post(&ak, &reference, &members, "retry-1", "shared text").unwrap();
        let mut message = StoredThreadMessage {
            seq: 1,
            context: post.context.clone(),
            copy: post.copies[1].clone(),
            pair_signature: post.pair_signature.clone(),
            created_at: 1,
        };
        message.context.commitment = "00".repeat(32);
        assert!(open_copy(&bk, &reference, &members, &message).is_err());
        message.context = post.context;
        message.copy.recipient = members[0].clone();
        assert!(open_copy(&bk, &reference, &members, &message).is_err());
    }

    #[test]
    fn strict_text_payload_rejects_empty_and_oversized_text() {
        let (a, ak) = member("owner-a", "agent-a", 1, 2);
        let (b, _) = member("owner-b", "agent-b", 3, 4);
        let reference = ThreadRef {
            creator_owner: a.owner.clone(),
            thread_id: uuid::Uuid::new_v4().to_string(),
        };
        assert!(create_post(&ak, &reference, &[a.clone(), b.clone()], "retry", "").is_err());
        assert!(create_post(
            &ak,
            &reference,
            &[a, b],
            "retry",
            &"x".repeat(MAX_TEXT_BYTES + 1)
        )
        .is_err());
    }

    #[test]
    fn escape_heavy_text_stays_within_binary_record_bound() {
        let (a, ak) = member("owner-a", "agent-a", 1, 2);
        let (b, bk) = member("owner-b", "agent-b", 3, 4);
        let reference = ThreadRef {
            creator_owner: a.owner.clone(),
            thread_id: uuid::Uuid::new_v4().to_string(),
        };
        let text = "\u{0001}".repeat(MAX_TEXT_BYTES);
        let members = [a, b];
        let post = create_post(&ak, &reference, &members, "control-text", &text).unwrap();
        assert!(post
            .copies
            .iter()
            .all(|copy| copy.ciphertext.len() <= (17 * 1024 + 16) * 2));
        for (index, keys) in [&ak, &bk].into_iter().enumerate() {
            let message = StoredThreadMessage {
                seq: 1,
                context: post.context.clone(),
                copy: post.copies[index].clone(),
                pair_signature: post.pair_signature.clone(),
                created_at: 1,
            };
            assert_eq!(
                open_copy(keys, &reference, &members, &message).unwrap(),
                text
            );
        }
    }

    #[test]
    fn private_retry_record_reuses_exact_pair_and_rejects_changed_text() {
        let (a, ak) = member("owner-a", "agent-a", 1, 2);
        let (b, _) = member("owner-b", "agent-b", 3, 4);
        let reference = ThreadRef {
            creator_owner: a.owner.clone(),
            thread_id: uuid::Uuid::new_v4().to_string(),
        };
        let members = [a, b];
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("retry.json");
        let first = persist_outgoing(
            &path,
            &ak,
            &reference,
            &members,
            "retry-key",
            "private text",
        )
        .unwrap();
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains("private text"));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let retry = persist_outgoing(
            &path,
            &ak,
            &reference,
            &members,
            "retry-key",
            "private text",
        )
        .unwrap();
        assert_eq!(retry, first);
        assert!(persist_outgoing(
            &path,
            &ak,
            &reference,
            &members,
            "retry-key",
            "changed text"
        )
        .is_err());

        let concurrent_path = directory.path().join("concurrent.json");
        let concurrent_ref = reference.clone();
        let concurrent_members = members.clone();
        let (first, second) = std::thread::scope(|scope| {
            let first_path = concurrent_path.clone();
            let first_ref = concurrent_ref.clone();
            let first_members = concurrent_members.clone();
            let first = scope.spawn(move || {
                let keys = Keys::new("owner-a".into(), "agent-a".into(), [1; 32], [2; 32]);
                persist_outgoing(
                    &first_path,
                    &keys,
                    &first_ref,
                    &first_members,
                    "same-key",
                    "same text",
                )
            });
            let second_path = concurrent_path.clone();
            let second_ref = concurrent_ref;
            let second_members = concurrent_members;
            let second = scope.spawn(move || {
                let keys = Keys::new("owner-a".into(), "agent-a".into(), [1; 32], [2; 32]);
                persist_outgoing(
                    &second_path,
                    &keys,
                    &second_ref,
                    &second_members,
                    "same-key",
                    "same text",
                )
            });
            (
                first.join().unwrap().unwrap(),
                second.join().unwrap().unwrap(),
            )
        });
        assert_eq!(first, second);
    }
}
