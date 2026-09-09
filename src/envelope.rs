//! The proposal envelope: the full unsigned destination-chain transaction
//! that rides along in the SputnikDAO proposal description so reviewers can
//! verify, byte for byte, what the MPC is being asked to sign.
//!
//! Layout of a description: the human-readable intent line first (so DAO UIs
//! show something legible), then a separator, then base64-encoded JSON.

use base64::Engine;
use color_eyre::eyre::WrapErr;

pub const SEPARATOR: &str = "---omni-envelope-v1---";
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
    /// Envelope format version
    pub omni: u32,
    /// Chain family, e.g. "evm"
    pub family: String,
    /// Chain registry key, e.g. "base"
    pub chain: String,
    /// Derivation path - determines the acting foreign account
    pub path: String,
    /// Full unsigned transaction (family-specific serialization)
    pub unsigned_tx: serde_json::Value,
    /// Human-readable intent, duplicated from the description head
    pub intent: String,
    #[serde(default)]
    pub meta: EnvelopeMeta,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EnvelopeMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<u64>,
    #[serde(default)]
    pub builder_version: String,
}

/// Warn above this envelope size: proposals live in DAO state forever and
/// storage is paid from the DAO's balance (~1 NEAR / 100 KB).
const SIZE_WARNING_BYTES: usize = 16 * 1024;

pub fn encode_description(envelope: &Envelope) -> color_eyre::eyre::Result<String> {
    let json = serde_json::to_vec(envelope).wrap_err("Failed to serialize the envelope")?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&json);
    let description = format!("{}\n\n{SEPARATOR}{encoded}", envelope.intent);
    if description.len() > SIZE_WARNING_BYTES {
        eprintln!(
            "Warning: the proposal description is {} KB; it is stored in the DAO's state \
             forever and paid from the DAO's balance.",
            description.len() / 1024
        );
    }
    Ok(description)
}

#[allow(dead_code)] // used by the upcoming `omni proposal review` and `omni broadcast` commands
pub fn decode_description(description: &str) -> Option<Envelope> {
    let encoded = description.split(SEPARATOR).nth(1)?.trim();
    let json = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    serde_json::from_slice(&json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let envelope = Envelope {
            omni: VERSION,
            family: "evm".to_string(),
            chain: "base".to_string(),
            path: "base-locker-admin".to_string(),
            unsigned_tx: serde_json::json!({"nonce": 17}),
            intent: "Pause Base locker".to_string(),
            meta: EnvelopeMeta {
                nonce: Some(17),
                builder_version: "0.1.0".to_string(),
            },
        };
        let description = encode_description(&envelope).unwrap();
        assert!(description.starts_with("Pause Base locker\n\n"));
        let decoded = decode_description(&description).unwrap();
        assert_eq!(decoded.chain, "base");
        assert_eq!(decoded.unsigned_tx["nonce"], 17);
    }
}
