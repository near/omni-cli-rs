//! The proposal envelope: the full unsigned destination-chain transaction
//! that rides along in the SputnikDAO proposal description so reviewers can
//! verify, byte for byte, what the MPC is being asked to sign.
//!
//! The description IS the envelope: compact single-line JSON of the schema
//! below - parseable by any tool, and free of `\n`/indentation noise when
//! viewed as a raw string in CLIs and DAO UIs.
//!
//! ```json
//! {
//!   "omni": 1,
//!   "intent": "Pause Base locker during incident #42",
//!   "family": "evm",
//!   "chain": "base",
//!   "path": "omni-1",
//!   "unsigned_tx": { ... },
//!   "meta": { "builder_version": "0.1.0" }
//! }
//! ```

use color_eyre::eyre::WrapErr;

pub const VERSION: u32 = 1;

/// Field order is the serialization order - keep the human-relevant fields
/// (version, intent, chain, path) at the top of the rendered JSON.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
    /// Envelope format version
    pub omni: u32,
    /// Human-readable intent for reviewers
    pub intent: String,
    /// Chain family, e.g. "evm"
    pub family: String,
    /// Chain registry key, e.g. "base"
    pub chain: String,
    /// Derivation path - determines the acting foreign account
    pub path: String,
    /// Full unsigned transaction (family-specific serialization)
    pub unsigned_tx: serde_json::Value,
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
    let description =
        serde_json::to_string(envelope).wrap_err("Failed to serialize the envelope")?;
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
    serde_json::from_str(description.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_is_readable_json() {
        let envelope = Envelope {
            omni: VERSION,
            intent: "Pause Base locker".to_string(),
            family: "evm".to_string(),
            chain: "base".to_string(),
            path: "omni-1".to_string(),
            unsigned_tx: serde_json::json!({"nonce": 17}),
            meta: EnvelopeMeta {
                nonce: Some(17),
                builder_version: "0.1.0".to_string(),
            },
        };
        let description = encode_description(&envelope).unwrap();

        // Compact single-line JSON, human-relevant fields near the top
        assert!(description.starts_with('{'));
        assert!(!description.contains('\n'));
        let omni_pos = description.find("\"omni\"").unwrap();
        let intent_pos = description.find("\"intent\"").unwrap();
        let tx_pos = description.find("\"unsigned_tx\"").unwrap();
        assert!(omni_pos < intent_pos && intent_pos < tx_pos);

        let decoded = decode_description(&description).unwrap();
        assert_eq!(decoded.chain, "base");
        assert_eq!(decoded.intent, "Pause Base locker");
        assert_eq!(decoded.unsigned_tx["nonce"], 17);

        assert!(decode_description("not an envelope").is_none());
    }
}
