//! MPC signer contract plumbing: derived keys, `sign` action construction,
//! and signature extraction from NEAR execution outcomes.
//!
//! Uses the chain-signatures v2 interface (near/mpc): key domains
//! (`domain_id`) select the curve, `payload_v2` is `{"Ecdsa"|"Eddsa": "<hex>"}`,
//! and responses are tagged with `"scheme"`.

use color_eyre::eyre::{ContextCompat, WrapErr, eyre};

use crate::chains::SignatureScheme;
use crate::config::MpcConfig;

/// The MPC signer contract for the selected network. The omni config takes
/// precedence; the near-cli-rs network connection's `mpc_contract_account_id`
/// is the fallback.
pub fn mpc_contract_id(
    mpc_config: &MpcConfig,
    network_config: &near_cli_rs::config::NetworkConfig,
) -> color_eyre::eyre::Result<near_primitives::types::AccountId> {
    if let Some(contract) = mpc_config.contracts.get(&network_config.network_name) {
        return contract
            .parse()
            .wrap_err("Invalid MPC signer contract account id in the omni config");
    }
    network_config
        .mpc_contract_account_id
        .clone()
        .wrap_err_with(|| {
            format!(
                "No MPC signer contract is configured for NEAR network '{}' \
                 (add it under [mpc.contracts] in the omni config)",
                network_config.network_name
            )
        })
}

pub fn domain_id(mpc_config: &MpcConfig, scheme: SignatureScheme) -> u64 {
    match scheme {
        SignatureScheme::Secp256k1 => mpc_config.secp256k1_domain_id,
        SignatureScheme::Ed25519 => mpc_config.ed25519_domain_id,
    }
}

/// Bridges a near-cli-rs network selection into a near-api NetworkConfig so
/// our own queries go through the same RPC endpoint the user selected.
pub fn to_near_api_network(
    network_config: &near_cli_rs::config::NetworkConfig,
) -> color_eyre::eyre::Result<near_api::NetworkConfig> {
    let base = if network_config.network_name == "mainnet" {
        near_api::NetworkConfig::mainnet()
    } else {
        near_api::NetworkConfig::testnet()
    };
    Ok(near_api::NetworkConfig {
        network_name: network_config.network_name.clone(),
        rpc_endpoints: vec![near_api::RPCEndpoint::new(
            network_config
                .rpc_url
                .as_ref()
                .parse()
                .wrap_err_with(|| format!("Invalid NEAR RPC url: {}", network_config.rpc_url))?,
        )],
        ..base
    })
}

pub fn block_on<F: std::future::Future>(future: F) -> color_eyre::eyre::Result<F::Output> {
    Ok(tokio::runtime::Runtime::new()
        .wrap_err("Failed to start a tokio runtime")?
        .block_on(future))
}

/// Fetches the public key derived from `(owner, path)` in the given key
/// domain via the MPC contract's `derived_public_key` view method. This is
/// the only place derivation math happens - and it happens on the contract,
/// so it can never drift from what the MPC actually signs with.
pub fn derived_public_key(
    network: &near_api::NetworkConfig,
    mpc_contract: &near_primitives::types::AccountId,
    owner: &near_primitives::types::AccountId,
    path: &str,
    domain_id: u64,
) -> color_eyre::eyre::Result<near_crypto::PublicKey> {
    #[derive(serde::Serialize)]
    struct DerivedPublicKeyArgs<'a> {
        path: &'a str,
        predecessor: &'a str,
        domain_id: u64,
    }

    let contract = near_api::Contract(
        mpc_contract
            .as_str()
            .parse()
            .wrap_err("Invalid MPC contract account id")?,
    );
    let args = DerivedPublicKeyArgs {
        path,
        predecessor: owner.as_str(),
        domain_id,
    };
    let response = block_on(
        contract
            .call_function("derived_public_key", args)
            .read_only::<String>()
            .fetch_from(network),
    )?
    .wrap_err_with(|| {
        format!(
            "Failed to fetch the derived public key from {mpc_contract} \
             (owner: {owner}, path: '{path}', domain: {domain_id})"
        )
    })?;

    response
        .data
        .parse()
        .wrap_err("MPC contract returned an unparseable public key")
}

pub fn secp256k1_bytes(public_key: &near_crypto::PublicKey) -> color_eyre::eyre::Result<[u8; 64]> {
    match public_key {
        near_crypto::PublicKey::SECP256K1(key) => {
            let mut bytes = [0u8; 64];
            bytes.copy_from_slice(key.as_ref());
            Ok(bytes)
        }
        near_crypto::PublicKey::ED25519(_) => {
            Err(eyre!("Expected a secp256k1 derived key, got: {public_key}"))
        }
    }
}

pub fn ed25519_bytes(public_key: &near_crypto::PublicKey) -> color_eyre::eyre::Result<[u8; 32]> {
    match public_key {
        near_crypto::PublicKey::ED25519(key) => Ok(key.0),
        near_crypto::PublicKey::SECP256K1(_) => {
            Err(eyre!("Expected an ed25519 derived key, got: {public_key}"))
        }
    }
}

/// The MPC-derived keys of both domains for `(owner, path)` - enough to
/// compute the derived address on every registered chain.
pub struct DerivedKeys {
    pub secp256k1: [u8; 64],
    pub ed25519: [u8; 32],
}

pub fn fetch_derived_keys(
    network_config: &near_cli_rs::config::NetworkConfig,
    mpc_config: &crate::config::MpcConfig,
    owner: &near_primitives::types::AccountId,
    path: &str,
) -> color_eyre::eyre::Result<DerivedKeys> {
    let mpc_contract = mpc_contract_id(mpc_config, network_config)?;
    let api_network = to_near_api_network(network_config)?;
    eprintln!("\nResolving the derived keys for {owner} / \"{path}\" via {mpc_contract} ...");

    let secp256k1 = secp256k1_bytes(&derived_public_key(
        &api_network,
        &mpc_contract,
        owner,
        path,
        domain_id(mpc_config, crate::chains::SignatureScheme::Secp256k1),
    )?)?;
    let ed25519 = ed25519_bytes(&derived_public_key(
        &api_network,
        &mpc_contract,
        owner,
        path,
        domain_id(mpc_config, crate::chains::SignatureScheme::Ed25519),
    )?)?;
    Ok(DerivedKeys { secp256k1, ed25519 })
}

/// Arguments of the MPC contract's `sign` method (v2 interface).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SignArgs {
    pub request: SignRequest,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SignRequest {
    pub path: String,
    pub payload_v2: SignPayload,
    pub domain_id: u64,
}

/// Externally tagged to match the contract: `{"Ecdsa": "<hex>"}` /
/// `{"Eddsa": "<hex>"}`.
#[derive(Debug, Clone, serde::Serialize)]
pub enum SignPayload {
    Ecdsa(String),
    Eddsa(String),
}

pub fn sign_request_args(
    payload: &[u8],
    scheme: SignatureScheme,
    path: &str,
    mpc_config: &MpcConfig,
) -> SignArgs {
    let payload_v2 = match scheme {
        SignatureScheme::Secp256k1 => SignPayload::Ecdsa(hex::encode(payload)),
        SignatureScheme::Ed25519 => SignPayload::Eddsa(hex::encode(payload)),
    };
    SignArgs {
        request: SignRequest {
            path: path.to_string(),
            payload_v2,
            domain_id: domain_id(mpc_config, scheme),
        },
    }
}

/// Budget for all `sign` actions together: they must fit in one NEAR
/// transaction / one `act_proposal` execution (300 TGas cap) with headroom.
const MAX_TOTAL_SIGN_GAS_TGAS: u64 = 280;

/// Gas to attach to each of `payload_count` sign actions: the configured
/// per-sign gas, shrunk so a multi-payload transaction (one signature per
/// UTXO input) still fits the 300 TGas cap. The live signer contracts
/// require >= 15 TGas per call.
pub fn sign_gas_per_action_tgas(mpc_config: &MpcConfig, payload_count: usize) -> u64 {
    mpc_config
        .sign_gas_tgas
        .min(MAX_TOTAL_SIGN_GAS_TGAS / payload_count.max(1) as u64)
}

/// The `sign` FunctionCall actions (one per payload), used directly
/// (sign-as-account) or mirrored inside a SputnikDAO proposal (sign-as-dao).
pub fn sign_actions(
    payloads: &[Vec<u8>],
    scheme: SignatureScheme,
    path: &str,
    mpc_config: &MpcConfig,
) -> Vec<near_primitives::transaction::Action> {
    let gas_tgas = sign_gas_per_action_tgas(mpc_config, payloads.len());
    payloads
        .iter()
        .map(|payload| {
            near_primitives::transaction::Action::FunctionCall(Box::new(
                near_primitives::transaction::FunctionCallAction {
                    method_name: "sign".to_string(),
                    args: serde_json::to_vec(&sign_request_args(payload, scheme, path, mpc_config))
                        .expect("SignArgs serialization is infallible"),
                    gas: near_primitives::gas::Gas::from_gas(
                        near_gas::NearGas::from_tgas(gas_tgas).as_gas(),
                    ),
                    deposit: near_token::NearToken::from_yoctonear(
                        mpc_config.sign_deposit_yoctonear,
                    ),
                },
            ))
        })
        .collect()
}

/// The signature the MPC contract resolves the `sign` yield with, as found in
/// receipt SuccessValues (chain-signatures v2, `#[serde(tag = "scheme")]`).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "scheme")]
pub enum MpcSignatureResponse {
    Secp256k1 {
        big_r: AffinePoint,
        s: Scalar,
        recovery_id: u8,
    },
    Ed25519 {
        signature: Vec<u8>,
    },
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AffinePoint {
    pub affine_point: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Scalar {
    pub scalar: String,
}

/// Legacy (pre-domains) response had the same secp256k1 fields, no tag.
#[derive(Debug, Clone, serde::Deserialize)]
struct LegacySecp256k1Response {
    big_r: AffinePoint,
    s: Scalar,
    recovery_id: u8,
}

pub fn parse_signature_response(bytes: &[u8]) -> Option<MpcSignatureResponse> {
    if let Ok(response) = serde_json::from_slice::<MpcSignatureResponse>(bytes) {
        return Some(response);
    }
    serde_json::from_slice::<LegacySecp256k1Response>(bytes)
        .ok()
        .map(|legacy| MpcSignatureResponse::Secp256k1 {
            big_r: legacy.big_r,
            s: legacy.s,
            recovery_id: legacy.recovery_id,
        })
}

/// Scans every receipt in the execution outcome for a value that parses as an
/// MPC signature response. Works both for direct `sign` calls and for
/// `act_proposal` executions that triggered `sign` deeper in the receipt tree.
pub fn extract_signature_responses(
    outcome: &near_primitives::views::FinalExecutionOutcomeView,
) -> Vec<MpcSignatureResponse> {
    let final_status_value = match &outcome.status {
        near_primitives::views::FinalExecutionStatus::SuccessValue(value) => Some(value.as_slice()),
        _ => None,
    };
    let receipt_values =
        outcome
            .receipts_outcome
            .iter()
            .filter_map(|receipt| match &receipt.outcome.status {
                near_primitives::views::ExecutionStatusView::SuccessValue(value) => {
                    Some(value.as_slice())
                }
                _ => None,
            });
    final_status_value
        .into_iter()
        .chain(receipt_values)
        .filter_map(parse_signature_response)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tagged_and_legacy_responses() {
        let tagged_secp = br#"{"scheme":"Secp256k1","big_r":{"affine_point":"02abcd"},"s":{"scalar":"ef01"},"recovery_id":1}"#;
        assert!(matches!(
            parse_signature_response(tagged_secp),
            Some(MpcSignatureResponse::Secp256k1 { recovery_id: 1, .. })
        ));

        let tagged_ed = br#"{"scheme":"Ed25519","signature":[1,2,3]}"#;
        assert!(matches!(
            parse_signature_response(tagged_ed),
            Some(MpcSignatureResponse::Ed25519 { .. })
        ));

        let legacy =
            br#"{"big_r":{"affine_point":"02abcd"},"s":{"scalar":"ef01"},"recovery_id":0}"#;
        assert!(matches!(
            parse_signature_response(legacy),
            Some(MpcSignatureResponse::Secp256k1 { recovery_id: 0, .. })
        ));

        assert!(parse_signature_response(b"42").is_none());
        assert!(parse_signature_response(b"\"just a string\"").is_none());
    }

    #[test]
    fn sign_args_use_v2_interface() {
        let config = MpcConfig::default();
        let secp = serde_json::to_value(sign_request_args(
            &[0xab; 32],
            SignatureScheme::Secp256k1,
            "p",
            &config,
        ))
        .unwrap();
        assert_eq!(secp["request"]["domain_id"], 0);
        assert_eq!(
            secp["request"]["payload_v2"]["Ecdsa"],
            hex::encode([0xab; 32])
        );

        let ed = serde_json::to_value(sign_request_args(
            &[0x01, 0x02],
            SignatureScheme::Ed25519,
            "p",
            &config,
        ))
        .unwrap();
        assert_eq!(ed["request"]["domain_id"], 1);
        assert_eq!(ed["request"]["payload_v2"]["Eddsa"], "0102");
    }
}
