//! Agent identity, derived by the broker from the presented credential.
//!
//! # Why this lives here and not in the gateway
//!
//! The Gate 0 exit condition is that the **broker, not the gateway, is the
//! authorization authority** (`docs/roadmap.md`). That is only true if the
//! broker establishes *who is calling* from its own inputs. So:
//!
//! - the token verifier lives here, in the privileged process, and the gateway
//!   never holds it — the gateway relays a token it cannot check and cannot
//!   mint;
//! - identity is the result of verifying that token against this store. There
//!   is no wire field carrying an agent id (see `agentbed_protocol::wire`), so
//!   a compromised or forged gateway has nothing to assert;
//! - a valid peer credential on the trusted socket grants nothing by itself.
//!
//! Token storage is a file of SHA-256 hashes. A fast hash is the right choice
//! for a ≥128-bit random bearer token — there is no low-entropy secret to
//! grind — but it is *not* right for anything a human chooses, so
//! [`TokenStore`] refuses short tokens rather than letting one be enrolled.
//! systemd-creds storage, rotation and expiry-on-issue land at Gate 2.

use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Minimum accepted token length in characters (~128 bits at base32/hex).
pub const MIN_TOKEN_CHARS: usize = 26;

/// The identity the broker resolved for a call.
///
/// Constructing one requires a verified token: there is no `AgentContext::new`
/// taking an agent id, so no code path can conjure an identity from a caller's
/// assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentContext {
    agent_id: String,
    manifest_ref: String,
}

impl AgentContext {
    /// The resolved agent id, as recorded in the ledger.
    #[must_use]
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Which manifest this identity is bound to.
    #[must_use]
    pub fn manifest_ref(&self) -> &str {
        &self.manifest_ref
    }
}

/// Why a token did not resolve to an identity.
///
/// All variants map to a single wire error (`unauthenticated`): telling a
/// caller *which* of these happened would confirm that a token exists, or that
/// it once did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// No entry matched the presented token.
    UnknownToken,
    /// The entry exists but was revoked.
    Revoked,
    /// The entry exists but has expired.
    Expired,
}

/// A credential to enroll.
#[derive(Debug, Clone)]
pub struct Enrollment {
    /// Identity this token resolves to.
    pub agent_id: String,
    /// Manifest the identity is bound to.
    pub manifest_ref: String,
    /// The token itself.
    pub token: String,
    /// Whether the credential has been revoked.
    pub revoked: bool,
    /// Unix-seconds expiry, if any.
    pub expires_at_unix: Option<u64>,
}

/// One enrolled credential.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenEntry {
    agent_id: String,
    manifest_ref: String,
    /// Lowercase hex SHA-256 of the token.
    token_sha256: String,
    #[serde(default)]
    revoked: bool,
    /// Unix seconds; `None` means no expiry, which Gate 2 removes.
    #[serde(default)]
    expires_at_unix: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenFile {
    tokens: Vec<TokenEntry>,
}

/// The broker's token store.
#[derive(Debug, Default)]
pub struct TokenStore {
    /// Keyed by token hash so lookup does not depend on caller-controlled order.
    by_hash: HashMap<[u8; 32], TokenEntry>,
}

impl TokenStore {
    /// Load from a JSON file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read token store {}: {e}", path.display()))?;
        Self::from_json(&raw)
    }

    /// Parse a token store from JSON text.
    pub fn from_json(raw: &str) -> Result<Self, String> {
        let parsed: TokenFile =
            serde_json::from_str(raw).map_err(|e| format!("malformed token store: {e}"))?;
        let mut by_hash = HashMap::new();
        for entry in parsed.tokens {
            let hash = decode_hex32(&entry.token_sha256)
                .ok_or_else(|| format!("token hash for {} is not 32 hex bytes", entry.agent_id))?;
            if by_hash.insert(hash, entry).is_some() {
                return Err("two entries share a token hash".to_owned());
            }
        }
        Ok(TokenStore { by_hash })
    }

    /// Build a store in memory, hashing the tokens. Test and provisioning use.
    pub fn from_pairs(
        entries: impl IntoIterator<Item = (String, String, String)>,
    ) -> Result<Self, String> {
        Self::from_enrollments(entries.into_iter().map(|(agent_id, manifest_ref, token)| {
            Enrollment {
                agent_id,
                manifest_ref,
                token,
                revoked: false,
                expires_at_unix: None,
            }
        }))
    }

    /// Build a store from full enrollments, including revoked and expiring ones.
    pub fn from_enrollments(entries: impl IntoIterator<Item = Enrollment>) -> Result<Self, String> {
        let mut by_hash = HashMap::new();
        for enrollment in entries {
            if enrollment.token.chars().count() < MIN_TOKEN_CHARS {
                return Err(format!(
                    "token for {} is shorter than {MIN_TOKEN_CHARS} characters",
                    enrollment.agent_id
                ));
            }
            let hash: [u8; 32] = Sha256::digest(enrollment.token.as_bytes()).into();
            let entry = TokenEntry {
                agent_id: enrollment.agent_id,
                manifest_ref: enrollment.manifest_ref,
                token_sha256: hex(&hash),
                revoked: enrollment.revoked,
                expires_at_unix: enrollment.expires_at_unix,
            };
            by_hash.insert(hash, entry);
        }
        Ok(TokenStore { by_hash })
    }

    /// Resolve a presented token to an identity.
    ///
    /// The lookup is by hash of the presented secret, so the comparison never
    /// walks the stored secret material and cannot leak a prefix by timing.
    pub fn resolve(&self, presented: &str) -> Result<AgentContext, AuthError> {
        if presented.chars().count() < MIN_TOKEN_CHARS {
            return Err(AuthError::UnknownToken);
        }
        let hash: [u8; 32] = Sha256::digest(presented.as_bytes()).into();
        let entry = self.by_hash.get(&hash).ok_or(AuthError::UnknownToken)?;
        if entry.revoked {
            return Err(AuthError::Revoked);
        }
        if let Some(expiry) = entry.expires_at_unix {
            if now_unix() >= expiry {
                return Err(AuthError::Expired);
            }
        }
        Ok(AgentContext {
            agent_id: entry.agent_id.clone(),
            manifest_ref: entry.manifest_ref.clone(),
        })
    }

    /// How many credentials are enrolled.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    /// Whether the store is empty. A broker with an empty store can serve
    /// nobody, which is the correct failure mode for a misconfigured deploy.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(u64::MAX)
}

fn decode_hex32(raw: &str) -> Option<[u8; 32]> {
    if raw.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        let start = i.checked_mul(2)?;
        let pair = raw.get(start..start.checked_add(2)?)?;
        *slot = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

fn hex(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{byte:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TOKEN_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn store() -> TokenStore {
        TokenStore::from_pairs([
            (
                "agent-a".to_owned(),
                "a.yaml".to_owned(),
                TOKEN_A.to_owned(),
            ),
            (
                "agent-b".to_owned(),
                "b.yaml".to_owned(),
                TOKEN_B.to_owned(),
            ),
        ])
        .expect("store builds")
    }

    #[test]
    fn resolves_each_token_to_its_own_identity() {
        let store = store();
        assert_eq!(store.resolve(TOKEN_A).unwrap().agent_id(), "agent-a");
        assert_eq!(store.resolve(TOKEN_B).unwrap().agent_id(), "agent-b");
    }

    #[test]
    fn refuses_unknown_short_and_empty_tokens() {
        let store = store();
        assert_eq!(
            store.resolve("cccccccccccccccccccccccccccccc"),
            Err(AuthError::UnknownToken)
        );
        assert_eq!(store.resolve(""), Err(AuthError::UnknownToken));
        assert_eq!(store.resolve("short"), Err(AuthError::UnknownToken));
    }

    #[test]
    fn refuses_revoked_and_expired_entries() {
        let raw = format!(
            r#"{{"tokens":[
                {{"agent_id":"revoked-agent","manifest_ref":"a.yaml","token_sha256":"{}","revoked":true}},
                {{"agent_id":"expired-agent","manifest_ref":"b.yaml","token_sha256":"{}","expires_at_unix":1}}
            ]}}"#,
            hex(&Sha256::digest(TOKEN_A.as_bytes()).into()),
            hex(&Sha256::digest(TOKEN_B.as_bytes()).into()),
        );
        let store = TokenStore::from_json(&raw).expect("store parses");
        assert_eq!(store.resolve(TOKEN_A), Err(AuthError::Revoked));
        assert_eq!(store.resolve(TOKEN_B), Err(AuthError::Expired));
    }

    #[test]
    fn refuses_to_enrol_a_low_entropy_token() {
        let result =
            TokenStore::from_pairs([("a".to_owned(), "a.yaml".to_owned(), "hunter2".to_owned())]);
        assert!(result.is_err());
    }

    #[test]
    fn identity_cannot_be_constructed_without_a_token() {
        // Compile-time property, asserted in prose because it cannot be
        // asserted in code: AgentContext has no public constructor, so the only
        // way to obtain one is TokenStore::resolve.
        let store = store();
        let ctx = store.resolve(TOKEN_A).unwrap();
        assert_eq!(ctx.manifest_ref(), "a.yaml");
    }
}
