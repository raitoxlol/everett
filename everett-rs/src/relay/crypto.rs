use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload as Aad},
    ChaCha20Poly1305,
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

use super::{bad, valid, Payload};
use crate::error::Result;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Device {
    pub owner: String,
    pub device_id: String,
    pub signing_key: String,
    pub encryption_key: String,
    pub signature: String,
}

impl Device {
    pub fn signed_bytes(&self) -> Vec<u8> {
        json!([
            "everett-device-v1",
            self.owner,
            self.device_id,
            self.signing_key,
            self.encryption_key
        ])
        .to_string()
        .into_bytes()
    }

    pub fn verify(&self) -> Result<()> {
        valid(&self.device_id)?;
        decode::<32>(&self.encryption_key)?;
        verify(&self.signing_key, &self.signature, &self.signed_bytes())
    }

    pub fn fingerprint(&self) -> String {
        format!(
            "SHA256:{}",
            hex::encode(Sha256::digest(self.signed_bytes()))
        )
    }
}

pub struct Keys {
    pub device: Device,
    signing: SigningKey,
    encryption: StaticSecret,
}

impl Keys {
    pub fn new(owner: String, device_id: String, signing: [u8; 32], encryption: [u8; 32]) -> Self {
        let signing = SigningKey::from_bytes(&signing);
        let encryption = StaticSecret::from(encryption);
        let mut device = Device {
            owner,
            device_id,
            signing_key: hex::encode(signing.verifying_key().to_bytes()),
            encryption_key: hex::encode(PublicKey::from(&encryption).to_bytes()),
            signature: String::new(),
        };
        device.signature = hex::encode(signing.sign(&device.signed_bytes()).to_bytes());
        Self {
            device,
            signing,
            encryption,
        }
    }

    pub fn sign(&self, bytes: &[u8]) -> String {
        hex::encode(self.signing.sign(bytes).to_bytes())
    }

    pub fn seal(
        &self,
        recipient: &Device,
        key: &str,
        payload: &Payload,
        now: i64,
        ttl: i64,
    ) -> Result<Envelope> {
        recipient.verify()?;
        if recipient.owner != self.device.owner {
            return Err(bad("peer belongs to another account"));
        }
        valid(key)?;
        payload.validate()?;
        let body = serde_json::to_vec(payload).map_err(bad)?;
        if body.len() > crate::peer::BODY_CAP {
            return Err(bad("relay plaintext exceeds 64 KiB"));
        }
        let secret = StaticSecret::random_from_rng(OsRng);
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        if !(1..=86_400_000).contains(&ttl) {
            return Err(bad("invalid TTL"));
        }
        let expires_at = now
            .checked_add(ttl)
            .ok_or_else(|| bad("invalid timestamp"))?;
        let mut envelope = Envelope {
            owner: self.device.owner.clone(),
            from_device: self.device.device_id.clone(),
            to_device: recipient.device_id.clone(),
            message_id: uuid::Uuid::new_v4().to_string(),
            idempotency_key: key.into(),
            created_at: now,
            expires_at,
            ephemeral_key: hex::encode(PublicKey::from(&secret).to_bytes()),
            nonce: hex::encode(nonce),
            ciphertext: String::new(),
            signature: String::new(),
        };
        let cipher = cipher(&secret, &recipient.encryption_key, &envelope.aad())?;
        envelope.ciphertext = hex::encode(
            cipher
                .encrypt(
                    (&nonce).into(),
                    Aad {
                        msg: &body,
                        aad: &envelope.aad(),
                    },
                )
                .map_err(|_| bad("encryption failed"))?,
        );
        envelope.signature = self.sign(&envelope.signed_bytes());
        Ok(envelope)
    }

    pub fn open(&self, sender: &Device, envelope: &Envelope, now: i64) -> Result<Payload> {
        sender.verify()?;
        if sender.owner != self.device.owner
            || envelope.owner != self.device.owner
            || envelope.to_device != self.device.device_id
            || envelope.from_device != sender.device_id
        {
            return Err(bad("relay account or device mismatch"));
        }
        envelope.validate(now)?;
        verify(
            &sender.signing_key,
            &envelope.signature,
            &envelope.signed_bytes(),
        )?;
        let cipher = cipher(&self.encryption, &envelope.ephemeral_key, &envelope.aad())?;
        let nonce = decode::<12>(&envelope.nonce)?;
        let encrypted = hex::decode(&envelope.ciphertext).map_err(bad)?;
        let body = cipher
            .decrypt(
                (&nonce).into(),
                Aad {
                    msg: &encrypted,
                    aad: &envelope.aad(),
                },
            )
            .map_err(|_| bad("relay ciphertext authentication failed"))?;
        if body.len() > crate::peer::BODY_CAP {
            return Err(bad("relay plaintext exceeds 64 KiB"));
        }
        let payload: Payload = serde_json::from_slice(&body).map_err(bad)?;
        payload.validate()?;
        Ok(payload)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Envelope {
    pub owner: String,
    pub from_device: String,
    pub to_device: String,
    pub message_id: String,
    pub idempotency_key: String,
    #[serde(deserialize_with = "timestamp")]
    pub created_at: i64,
    #[serde(deserialize_with = "timestamp")]
    pub expires_at: i64,
    pub ephemeral_key: String,
    pub nonce: String,
    pub ciphertext: String,
    pub signature: String,
}

fn timestamp<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<i64, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let number = value
        .as_f64()
        .ok_or_else(|| serde::de::Error::custom("invalid timestamp"))?;
    if !number.is_finite() || number.fract() != 0.0 || number.abs() > 9_007_199_254_740_991.0 {
        return Err(serde::de::Error::custom("timestamp must be a safe integer"));
    }
    Ok(number as i64)
}

impl Envelope {
    pub fn aad(&self) -> Vec<u8> {
        json!([
            "everett-relay-v1",
            self.owner,
            self.from_device,
            self.to_device,
            self.message_id,
            self.idempotency_key,
            self.created_at,
            self.expires_at,
            self.ephemeral_key,
            self.nonce
        ])
        .to_string()
        .into_bytes()
    }

    pub fn signed_bytes(&self) -> Vec<u8> {
        json!([String::from_utf8(self.aad()).unwrap(), self.ciphertext])
            .to_string()
            .into_bytes()
    }

    pub fn validate(&self, now: i64) -> Result<()> {
        for id in [
            &self.from_device,
            &self.to_device,
            &self.message_id,
            &self.idempotency_key,
        ] {
            valid(id)?;
        }
        let lifetime = self.expires_at.checked_sub(self.created_at).unwrap_or(0);
        if self.expires_at <= now
            || self.created_at > now.saturating_add(300_000)
            || !(1..=86_400_000).contains(&lifetime)
        {
            return Err(bad(
                "relay envelope expired or invalid TTL (maximum 24 hours)",
            ));
        }
        if self.ciphertext.len() > (crate::peer::BODY_CAP + 16) * 2 {
            return Err(bad("relay ciphertext exceeds body cap"));
        }
        decode::<32>(&self.ephemeral_key)?;
        decode::<12>(&self.nonce)?;
        Ok(())
    }
}

fn cipher(secret: &StaticSecret, public: &str, context: &[u8]) -> Result<ChaCha20Poly1305> {
    let shared = secret.diffie_hellman(&PublicKey::from(decode::<32>(public)?));
    if !shared.was_contributory() {
        return Err(bad("invalid X25519 public key"));
    }
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(b"everett-relay-v1"), shared.as_bytes())
        .expand(context, &mut key)
        .map_err(bad)?;
    Ok(ChaCha20Poly1305::new((&key).into()))
}

pub fn decode<const N: usize>(value: &str) -> Result<[u8; N]> {
    hex::decode(value)
        .map_err(bad)?
        .try_into()
        .map_err(|_| bad("invalid key or nonce length"))
}

pub fn verify(public: &str, signature: &str, bytes: &[u8]) -> Result<()> {
    let key = VerifyingKey::from_bytes(&decode::<32>(public)?).map_err(bad)?;
    key.verify_strict(bytes, &Signature::from_bytes(&decode::<64>(signature)?))
        .map_err(|_| bad("invalid Ed25519 signature"))
}
