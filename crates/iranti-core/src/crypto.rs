//! NIP-AE-compatible engram crypto + envelope construction.
//!
//! Mirrors `buzz-core::engram` byte-for-byte on the core envelope (d-tag HMAC,
//! NIP-44 conversation key, `p`-tag owner) so events produced here are valid
//! NIP-AE engrams that Buzz relays and `buzz mem` can read. The decrypted body
//! extends the NIP-AE memory body with optional companion fields
//! (`ns`, `salience`, `pin`, `ttl`, `prov`) — permitted by NIP-AE's
//! unknown-fields-permissive rule.

use anyhow::{bail, Result};
use hmac::{Hmac, Mac};
use nostr::nips::nip44::{self, v2::ConversationKey, Version};
use nostr::{Event, EventBuilder, Kind, Keys, PublicKey, SecretKey, Tag};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// NIP-AE kind for agent engrams.
pub const KIND_AGENT_ENGRAM: u32 = 30174;
/// NIP-AE d-tag domain prefix (must never change for v1).
pub const D_TAG_DOMAIN: &[u8] = b"agent-memory/v1/d-tag";
/// NIP-44 plaintext cap.
pub const NIP44_PLAINTEXT_MAX: usize = 65_535;

/// Iranti companion kinds (proposed NIP-IR; require relay allowlist entry).
pub const KIND_RECALL_INDEX: u32 = 36000;
pub const KIND_MEMORY_GRANT: u32 = 36001;
pub const KIND_DREAM_DIGEST: u32 = 36002;
pub const KIND_ECHO_REPORT: u32 = 36003;

/// Derive the NIP-44 v2 conversation key for (my_seckey, their_pubkey).
/// Symmetric: derive(a_sk, b_pk) == derive(b_sk, a_pk).
pub fn conversation_key(my_seckey: &SecretKey, their_pubkey: &PublicKey) -> ConversationKey {
    ConversationKey::derive(my_seckey, their_pubkey)
        .expect("valid keys produce conversation key")
}

/// NIP-AE d-tag: lower_hex(HMAC-SHA256(K_c, "agent-memory/v1/d-tag" || 0x00 || slug)).
pub fn d_tag(k_c: &ConversationKey, slug: &str) -> String {
    let mut mac = <Hmac<Sha256> as hmac::digest::KeyInit>::new_from_slice(k_c.as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(D_TAG_DOMAIN);
    mac.update(&[0u8]);
    mac.update(slug.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Iranti extended memory body: NIP-AE `{slug, value}` + companion fields.
///
/// Serialization keeps `slug` first so the core prefix is byte-identical to
/// NIP-AE reference vectors (`{"slug":"…","value":"…"}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemBody {
    pub slug: String,
    pub value: String,
    /// Coarse recall axis (e.g. "design", "ops"). Non-sensitive by convention.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ns: Option<String>,
    /// Salience 0-100 (attention weight for recall ranking).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub salience: Option<u8>,
    /// Pinned memories resist decay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin: Option<bool>,
    /// TTL in seconds from creation (0/none = forever).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<i64>,
    /// Provenance pointer, e.g. "buzz#42#<event_id>".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prov: Option<String>,
}

impl MemBody {
    pub fn new(slug: String, value: String) -> Self {
        Self { slug, value, ns: None, salience: None, pin: None, ttl: None, prov: None }
    }

    /// Serialize body to the JSON bytes that get NIP-44 encrypted.
    pub fn to_json_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("body serialization is infallible")
    }

    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        let b: MemBody = serde_json::from_slice(bytes)?;
        if b.slug == "core" {
            bail!("core body is not a memory body");
        }
        if !b.slug.starts_with("mem/") {
            bail!("invalid slug {:?}: expected mem/...", b.slug);
        }
        Ok(b)
    }
}

/// Encrypt a body under the agent<->owner conversation key (NIP-44 v2).
/// Mirrors buzz-core::engram::build_event exactly (SecretKey + Version::V2).
pub fn encrypt_body(keys: &Keys, owner_pk: &PublicKey, body: &MemBody) -> Result<String> {
    let plain = body.to_json_bytes();
    if plain.len() > NIP44_PLAINTEXT_MAX {
        bail!("body exceeds {NIP44_PLAINTEXT_MAX}-byte plaintext limit ({} bytes)", plain.len());
    }
    let sk = keys.secret_key();
    let text = String::from_utf8(plain).expect("body is valid UTF-8");
    Ok(nip44::encrypt(sk, owner_pk, &text, Version::V2)?)
}

/// Decrypt a body. `counterparty_pk` is the *other* party of the pair:
/// agent reads with (agent keys, owner pk); owner reads with (owner keys, agent pk).
pub fn decrypt_body(keys: &Keys, counterparty_pk: &PublicKey, content: &str) -> Result<MemBody> {
    let sk = keys.secret_key();
    let plain = nip44::decrypt(sk, counterparty_pk, content)?;
    MemBody::from_json_bytes(plain.as_bytes())
}

/// Build a signed NIP-AE kind:30174 engram event.
///
/// Envelope: exactly one `d` tag (64-hex HMAC), exactly one `p` tag (owner).
/// `created_at` is caller-supplied so writers can enforce NIP-AE monotonicity.
pub fn build_engram_event(
    keys: &Keys,
    owner_pk: &PublicKey,
    slug: &str,
    body: &MemBody,
    created_at: u64,
) -> Result<Event> {
    let sk = keys.secret_key();
    let k_c = conversation_key(sk, owner_pk);
    let d = d_tag(&k_c, slug);
    let content = encrypt_body(keys, owner_pk, body)?;
    let tags = vec![
        Tag::parse(["d", &d])?,
        Tag::parse(["p", &owner_pk.to_hex()])?,
    ];
    let ev = EventBuilder::new(Kind::Custom(KIND_AGENT_ENGRAM as u16), content)
        .tags(tags)
        .custom_created_at(nostr::Timestamp::from(created_at))
        .sign_with_keys(keys)?;
    Ok(ev)
}

/// Recompute the d-tag an event *should* carry for a slug, for conformance checks.
pub fn expected_d_tag(keys: &Keys, owner_pk: &PublicKey, slug: &str) -> Result<String> {
    let sk = keys.secret_key();
    Ok(d_tag(&conversation_key(sk, owner_pk), slug))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::Keys;

    fn pair() -> (Keys, Keys) {
        (Keys::generate(), Keys::generate())
    }

    #[test]
    fn conversation_key_is_symmetric() {
        let (a, b) = pair();
        let k1 = conversation_key(a.secret_key(), &b.public_key());
        let k2 = conversation_key(b.secret_key(), &a.public_key());
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn d_tag_is_64_hex_and_opaque() {
        let (a, b) = pair();
        let k = conversation_key(a.secret_key(), &b.public_key());
        let d = d_tag(&k, "mem/design/xyz");
        assert_eq!(d.len(), 64);
        assert!(d.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(d, d_tag(&k, "mem/ops/xyz"));
        assert!(!d.contains("design"));
    }

    #[test]
    fn roundtrip_body_and_event() {
        let (agent, owner) = pair();
        let mut body = MemBody::new(
            "mem/design/note1".into(),
            "The Buzz relay gates kinds in ingest.rs".into(),
        );
        body.ns = Some("design".into());
        body.salience = Some(90);
        body.pin = Some(true);
        let ev = build_engram_event(
            &agent,
            &owner.public_key(),
            "mem/design/note1",
            &body,
            1_700_000_000,
        )
        .unwrap();
        assert_eq!(ev.kind.as_u16() as u32, KIND_AGENT_ENGRAM);
        assert_eq!(ev.tags.iter().filter(|t| t.kind().as_str() == "d").count(), 1);
        assert_eq!(ev.tags.iter().filter(|t| t.kind().as_str() == "p").count(), 1);
        let d = expected_d_tag(&agent, &owner.public_key(), "mem/design/note1").unwrap();
        let d_ev = ev
            .tags
            .iter()
            .find(|t| t.kind().as_str() == "d")
            .unwrap()
            .content()
            .unwrap_or("")
            .to_string();
        assert_eq!(d_ev, d);
        // agent-side decryption with owner pk roundtrips
        let dec = decrypt_body(&agent, &owner.public_key(), &ev.content).unwrap();
        assert_eq!(dec.slug, body.slug);
        assert_eq!(dec.value, body.value);
        assert_eq!(dec.ns.as_deref(), Some("design"));
        assert_eq!(dec.pin, Some(true));
        // owner-side decryption with agent pk roundtrips
        let dec_owner = decrypt_body(&owner, &agent.public_key(), &ev.content).unwrap();
        assert_eq!(dec_owner.value, body.value);
        // signature verifies
        assert!(ev.verify_signature());
    }

    #[test]
    fn body_json_prefix_matches_nip_ae_reference_shape() {
        let body = MemBody::new("mem/design/x".into(), "hello".to_string());
        let bytes = body.to_json_bytes();
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with(r#"{"slug":"mem/design/x","value":"hello"}"#));
    }
}
