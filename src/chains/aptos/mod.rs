//! Aptos chain family: ed25519 payloads built with omni-transaction-rs Aptos
//! builders. The MPC signs the full signing message
//! (`sha3_256("APTOS::RawTransaction") || bcs(raw_txn)`) via its ed25519
//! key domain.

pub mod rpc;

use color_eyre::eyre::{ContextCompat, WrapErr, eyre};
use omni_transaction::aptos::AptosTransaction;
use omni_transaction::aptos::types::{
    AccountAddress, Ed25519PublicKey, Ed25519Signature, EntryFunction, Identifier, ModuleId,
    TransactionPayload,
};
use sha3::{Digest, Sha3_256};

use crate::chains::{BuiltTransaction, ChainAdapter, ExecutionLatency, SignatureScheme};
use crate::config::ResolvedChain;
use crate::mpc::MpcSignatureResponse;

pub const FAMILY: &str = "aptos";

/// Single-signer ed25519 scheme byte in the authentication-key preimage.
const ED25519_SCHEME: u8 = 0x00;

const MAX_GAS_AMOUNT: u64 = 2_000;
const IMMEDIATE_EXPIRATION_SECS: u64 = 10 * 60;
const GOVERNANCE_EXPIRATION_SECS: u64 = 14 * 24 * 60 * 60;

#[derive(Debug, Clone)]
pub enum AptosActionSpec {
    /// APT transfer via `0x1::aptos_account::transfer` (creates the recipient
    /// account if needed).
    Transfer { to: [u8; 32], octas: u64 },
}

/// What goes into the envelope: the raw transaction plus the sender's public
/// key, which the Ed25519 authenticator needs at assembly time.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AptosUnsignedPayload {
    pub tx: AptosTransaction,
    pub sender_public_key: String,
}

pub struct AptosAdapter {
    pub spec: AptosActionSpec,
}

/// Aptos authentication key / account address of an ed25519 public key:
/// `sha3_256(public_key || 0x00)`.
pub fn address_from_derived_pk(public_key: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha3_256::new();
    hasher.update(public_key);
    hasher.update([ED25519_SCHEME]);
    hasher.finalize().into()
}

fn address_hex(address: [u8; 32]) -> String {
    format!("0x{}", hex::encode(address))
}

impl ChainAdapter for AptosAdapter {
    fn family(&self) -> &'static str {
        FAMILY
    }

    fn scheme(&self) -> SignatureScheme {
        SignatureScheme::Ed25519
    }

    fn derived_address(
        &self,
        public_key: &near_crypto::PublicKey,
    ) -> color_eyre::eyre::Result<String> {
        let pk = crate::mpc::ed25519_bytes(public_key)?;
        Ok(address_hex(address_from_derived_pk(&pk)))
    }

    fn build(
        &self,
        chain: &ResolvedChain,
        derived_public_key: &near_crypto::PublicKey,
        owner: &str,
        derivation_path: &str,
        latency: ExecutionLatency,
    ) -> color_eyre::eyre::Result<BuiltTransaction> {
        let pk = crate::mpc::ed25519_bytes(derived_public_key)?;
        let sender = address_from_derived_pk(&pk);
        let sender_hex = address_hex(sender);

        let (chain_id, ledger_time_secs) = rpc::ledger_info(&chain.rpc_url)
            .wrap_err_with(|| format!("Failed to fetch ledger info from {}", chain.rpc_url))?;
        let sequence_number = rpc::sequence_number(&chain.rpc_url, &sender_hex)?;
        let (gas_price, prioritized_gas_price) = rpc::estimate_gas_price(&chain.rpc_url)?;
        let balance = rpc::apt_balance(&chain.rpc_url, &sender_hex);

        let (gas_unit_price, expiration_offset_secs, validity_note) = match latency {
            ExecutionLatency::Immediate => (
                gas_price,
                IMMEDIATE_EXPIRATION_SECS,
                "expires in 10 minutes".to_string(),
            ),
            ExecutionLatency::Governance => (
                prioritized_gas_price,
                GOVERNANCE_EXPIRATION_SECS,
                format!(
                    "expires in 14 days (DAO voting window); assumes sequence number \
                     {sequence_number} is still unused at execution time"
                ),
            ),
        };
        let expiration_timestamp_secs = ledger_time_secs + expiration_offset_secs;

        let (payload, summary, required_octas) = match &self.spec {
            AptosActionSpec::Transfer { to, octas } => (
                TransactionPayload::EntryFunction(EntryFunction::new(
                    ModuleId::new(
                        AccountAddress::ONE,
                        Identifier::new("aptos_account")
                            .map_err(|err| eyre!("Invalid identifier: {err:?}"))?,
                    ),
                    Identifier::new("transfer")
                        .map_err(|err| eyre!("Invalid identifier: {err:?}"))?,
                    vec![],
                    vec![to.to_vec(), octas.to_le_bytes().to_vec()],
                )),
                format!(
                    "transfer {} to {}",
                    format_native(*octas, chain),
                    address_hex(*to)
                ),
                octas + MAX_GAS_AMOUNT * gas_unit_price,
            ),
        };

        let tx = AptosTransaction {
            sender: AccountAddress::from_hex(&sender_hex)
                .map_err(|err| eyre!("Invalid sender address: {err:?}"))?,
            sequence_number,
            payload,
            max_gas_amount: MAX_GAS_AMOUNT,
            gas_unit_price,
            expiration_timestamp_secs,
            chain_id,
        };
        let signing_payload = tx.build_for_signing();

        let balance_note = if balance < required_octas {
            format!(
                "\n   WARNING: balance {} is below the required {} (amount + max gas) - \
                 fund the derived address first",
                format_native(balance, chain),
                format_native(required_octas, chain),
            )
        } else {
            String::new()
        };

        let display = format!(
            "\n\
             Unsigned {chain_key} transaction (Aptos chain id {chain_id}, NEAR {near_network}):\n\
             ------------------------------------------------------------\n\
             action:            {summary}\n\
             sender (from):     {sender_hex} (derived: {owner} / \"{derivation_path}\")\n\
             balance:           {balance}{balance_note}\n\
             sequence number:   {sequence_number}\n\
             gas:               max {MAX_GAS_AMOUNT} units x {gas_unit_price} octas/unit \
             (max {max_gas_cost})\n\
             expiration:        unix {expiration_timestamp_secs} ({validity_note})\n\
             signing payload:   {payload_len} bytes (salted message, signed as-is by the MPC)\n\
             ------------------------------------------------------------",
            chain_key = chain.chain_key,
            near_network = chain.near_network,
            balance = format_native(balance, chain),
            max_gas_cost = format_native(MAX_GAS_AMOUNT * gas_unit_price, chain),
            payload_len = signing_payload.len(),
        );

        let mut unsigned_tx = serde_json::to_value(AptosUnsignedPayload {
            tx,
            sender_public_key: hex::encode(pk),
        })?;
        prettify_entry_function_args(&mut unsigned_tx);

        Ok(BuiltTransaction {
            unsigned_tx,
            payloads: vec![signing_payload],
            display,
        })
    }
}

/// Recomputes the MPC signing payload from an envelope's unsigned tx -
/// the byte-equality half of `proposal review`.
pub(crate) fn signing_payloads_from_envelope(
    unsigned_tx: &serde_json::Value,
) -> color_eyre::eyre::Result<Vec<Vec<u8>>> {
    let mut unsigned_tx = unsigned_tx.clone();
    unprettify_entry_function_args(&mut unsigned_tx)?;
    let payload: AptosUnsignedPayload = serde_json::from_value(unsigned_tx)
        .wrap_err("Failed to deserialize the unsigned Aptos transaction")?;
    Ok(vec![payload.tx.build_for_signing()])
}

/// Combines the unsigned Aptos transaction with the MPC signature and
/// broadcasts it; returns the transaction hash.
pub fn assemble_and_broadcast(
    chain: &ResolvedChain,
    unsigned_tx: &serde_json::Value,
    signatures: &[MpcSignatureResponse],
) -> color_eyre::eyre::Result<String> {
    let mut unsigned_tx = unsigned_tx.clone();
    unprettify_entry_function_args(&mut unsigned_tx)?;
    let payload: AptosUnsignedPayload = serde_json::from_value(unsigned_tx)
        .wrap_err("Failed to deserialize the unsigned Aptos transaction")?;
    let response = signatures
        .first()
        .wrap_err("No MPC signature available to assemble")?;
    let signature = ed25519_signature_from_mpc(response)?;
    let public_key = Ed25519PublicKey::from_hex(&payload.sender_public_key)
        .map_err(|err| eyre!("Invalid sender public key in the envelope: {err:?}"))?;
    let signed_tx = payload.tx.build_with_signature(&public_key, &signature);
    rpc::submit_transaction(&chain.rpc_url, &signed_tx)
}

/// Envelope prettification: entry-function args are BCS byte blobs that the
/// upstream serde emits as number arrays; show them as hex (arg #0 of a
/// transfer is the recipient address).
fn prettify_entry_function_args(unsigned_tx: &mut serde_json::Value) {
    if let Some(args) = unsigned_tx
        .pointer_mut("/tx/payload/EntryFunction/args")
        .and_then(serde_json::Value::as_array_mut)
    {
        for arg in args {
            crate::chains::bytes_array_to_hex(arg);
        }
    }
}

fn unprettify_entry_function_args(
    unsigned_tx: &mut serde_json::Value,
) -> color_eyre::eyre::Result<()> {
    if let Some(args) = unsigned_tx
        .pointer_mut("/tx/payload/EntryFunction/args")
        .and_then(serde_json::Value::as_array_mut)
    {
        for arg in args {
            crate::chains::hex_to_bytes_array(arg)?;
        }
    }
    Ok(())
}

pub fn ed25519_signature_from_mpc(
    response: &MpcSignatureResponse,
) -> color_eyre::eyre::Result<Ed25519Signature> {
    let MpcSignatureResponse::Ed25519 { signature } = response else {
        return Err(eyre!(
            "Expected an ed25519 MPC signature for an Aptos chain, got a secp256k1 one"
        ));
    };
    let bytes: [u8; 64] = signature.as_slice().try_into().map_err(|_| {
        eyre!(
            "MPC ed25519 signature must be 64 bytes, got {}",
            signature.len()
        )
    })?;
    Ok(Ed25519Signature(bytes))
}

fn format_native(octas: u64, chain: &ResolvedChain) -> String {
    crate::types::format_move_style_amount(
        octas,
        &chain.symbol,
        "octas",
        10u64.pow(u32::from(chain.decimals)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey, Verifier};

    /// Round-trip through a real ed25519 key: build a transfer, sign the
    /// salted signing message the way the MPC does, assemble, and verify the
    /// broadcast layout (`bcs(raw) || 0x00 || 0x20 || pk || 0x40 || sig`).
    #[test]
    fn ed25519_signature_assembles_and_verifies() {
        let signing_key = SigningKey::from_bytes(&[11u8; 32]);
        let pk = signing_key.verifying_key().to_bytes();
        let sender = address_from_derived_pk(&pk);

        let tx = AptosTransaction {
            sender: AccountAddress::from_hex(&hex::encode(sender)).unwrap(),
            sequence_number: 7,
            payload: TransactionPayload::EntryFunction(EntryFunction::new(
                ModuleId::new(
                    AccountAddress::ONE,
                    Identifier::new("aptos_account").unwrap(),
                ),
                Identifier::new("transfer").unwrap(),
                vec![],
                vec![vec![0xddu8; 32], 1_000u64.to_le_bytes().to_vec()],
            )),
            max_gas_amount: MAX_GAS_AMOUNT,
            gas_unit_price: 100,
            expiration_timestamp_secs: 1_800_000_000,
            chain_id: 2,
        };

        let signing_message = tx.build_for_signing();
        // Salt prefix: sha3_256("APTOS::RawTransaction")
        assert_eq!(
            hex::encode(&signing_message[..32]),
            "b5e97db07fa0bd0e5598aa3643a9bc6f6693bddc1a9fec9e674a461eaa00b193"
        );

        let signature = signing_key.sign(&signing_message);
        signing_key
            .verifying_key()
            .verify(&signing_message, &signature)
            .unwrap();

        let response = MpcSignatureResponse::Ed25519 {
            signature: signature.to_bytes().to_vec(),
        };
        let aptos_signature = ed25519_signature_from_mpc(&response).unwrap();
        let signed_tx = tx.build_with_signature(&Ed25519PublicKey(pk), &aptos_signature);

        // Authenticator tail: 0x00 (ed25519 variant), 0x20, pk, 0x40, sig
        let raw_len = tx.to_bcs_bytes().len();
        assert_eq!(signed_tx.len(), raw_len + 1 + 1 + 32 + 1 + 64);
        assert_eq!(signed_tx[raw_len], 0x00);
        assert_eq!(signed_tx[raw_len + 1], 0x20);
        assert_eq!(&signed_tx[raw_len + 2..raw_len + 34], pk.as_slice());
        assert_eq!(signed_tx[raw_len + 34], 0x40);
        assert_eq!(&signed_tx[raw_len + 35..], signature.to_bytes().as_slice());
    }

    /// The envelope form shows BCS args as hex and round-trips exactly.
    #[test]
    fn envelope_args_prettify_and_round_trip() {
        let tx = AptosTransaction {
            sender: AccountAddress::from_hex("0xa550c18").unwrap(),
            sequence_number: 0,
            payload: TransactionPayload::EntryFunction(EntryFunction::new(
                ModuleId::new(
                    AccountAddress::ONE,
                    Identifier::new("aptos_account").unwrap(),
                ),
                Identifier::new("transfer").unwrap(),
                vec![],
                vec![vec![0xddu8; 32], 1_000u64.to_le_bytes().to_vec()],
            )),
            max_gas_amount: MAX_GAS_AMOUNT,
            gas_unit_price: 100,
            expiration_timestamp_secs: 1_800_000_000,
            chain_id: 2,
        };
        let expected_payload = tx.build_for_signing();

        let mut unsigned_tx = serde_json::to_value(AptosUnsignedPayload {
            tx,
            sender_public_key: "00".repeat(32),
        })
        .unwrap();
        prettify_entry_function_args(&mut unsigned_tx);
        let args = &unsigned_tx["tx"]["payload"]["EntryFunction"]["args"];
        assert_eq!(args[0], format!("0x{}", "dd".repeat(32)));

        unprettify_entry_function_args(&mut unsigned_tx).unwrap();
        let restored: AptosUnsignedPayload = serde_json::from_value(unsigned_tx).unwrap();
        assert_eq!(restored.tx.build_for_signing(), expected_payload);
    }
}
