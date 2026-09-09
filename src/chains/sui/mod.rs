//! Sui chain family: ed25519 payloads built with omni-transaction-rs Sui
//! builders. The MPC signs the 32-byte Blake2b-256 intent digest verbatim via
//! its ed25519 key domain (Sui validators verify ed25519 over the digest).

pub mod rpc;

use color_eyre::eyre::{ContextCompat, WrapErr, eyre};
use omni_transaction::sui::SuiTransaction;
use omni_transaction::sui::types::{
    Argument, CallArg, Command, GasData, ObjectDigest, ObjectRef, ProgrammableTransaction,
    SignatureScheme as SuiSignatureScheme, SuiAddress, SuiSignature, TransactionExpiration,
    TransactionKind,
};
use omni_transaction::sui::utils::derive_sui_address;

use crate::chains::{BuiltTransaction, ChainAdapter, ExecutionLatency, SignatureScheme};
use crate::config::ResolvedChain;
use crate::mpc::MpcSignatureResponse;

pub const FAMILY: &str = "sui";

/// Default gas budget: 0.01 SUI, far above a simple transfer's cost; the
/// unused part is refunded to the gas coin.
const GAS_BUDGET_MIST: u64 = 10_000_000;

#[derive(Debug, Clone)]
pub enum SuiActionSpec {
    /// Native SUI transfer: SplitCoins from the gas coin + TransferObjects.
    Transfer { to: [u8; 32], mist: u64 },
}

/// What goes into the envelope: the transaction plus the sender's public key,
/// which the Sui signature envelope (`flag || sig || pk`) needs at assembly.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SuiUnsignedPayload {
    pub tx: SuiTransaction,
    pub sender_public_key: String,
}

pub struct SuiAdapter {
    pub spec: SuiActionSpec,
}

impl ChainAdapter for SuiAdapter {
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
        Ok(derive_sui_address(SuiSignatureScheme::Ed25519, &pk).to_hex())
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
        let sender = derive_sui_address(SuiSignatureScheme::Ed25519, &pk);
        let sender_hex = sender.to_hex();

        let gas_price = rpc::reference_gas_price(&chain.rpc_url)
            .wrap_err_with(|| format!("Failed to fetch the gas price from {}", chain.rpc_url))?;

        let (amount_mist, to) = match &self.spec {
            SuiActionSpec::Transfer { to, mist } => (*mist, *to),
        };
        let required = amount_mist + GAS_BUDGET_MIST;

        // Select gas coins (largest first) until they cover amount + budget;
        // the transfer amount is split off the (merged) gas coin.
        let mut coins = rpc::sui_coins(&chain.rpc_url, &sender_hex)?;
        coins.sort_by_key(|coin| std::cmp::Reverse(coin.balance));
        let total_balance: u64 = coins.iter().map(|coin| coin.balance).sum();
        let mut payment = Vec::new();
        let mut covered = 0u64;
        for coin in &coins {
            if covered >= required {
                break;
            }
            covered += coin.balance;
            payment.push(ObjectRef::new(
                SuiAddress::from_hex(&coin.object_id)
                    .map_err(|err| eyre!("Invalid coin object id from the RPC: {err:?}"))?,
                coin.version,
                ObjectDigest::from_base58(&coin.digest_base58)
                    .map_err(|err| eyre!("Invalid coin digest from the RPC: {err:?}"))?,
            ));
        }
        if covered < required {
            return Err(eyre!(
                "The derived address {sender_hex} holds {} but this transaction needs {} \
                 (amount + gas budget) - fund the derived address first.",
                format_native(total_balance, chain),
                format_native(required, chain),
            ));
        }

        let tx = SuiTransaction {
            kind: TransactionKind::ProgrammableTransaction(ProgrammableTransaction {
                inputs: vec![
                    CallArg::pure_u64(amount_mist),
                    CallArg::pure_address(SuiAddress(to)),
                ],
                commands: vec![
                    Command::SplitCoins {
                        coin: Argument::GasCoin,
                        amounts: vec![Argument::Input(0)],
                    },
                    Command::TransferObjects {
                        objects: vec![Argument::Result(0)],
                        address: Argument::Input(1),
                    },
                ],
            }),
            sender,
            gas_data: GasData {
                payment,
                owner: sender,
                price: gas_price,
                budget: GAS_BUDGET_MIST,
            },
            expiration: TransactionExpiration::None,
        };

        let signing_digest = tx.build_for_signing();

        let validity_note = match latency {
            ExecutionLatency::Immediate => "no expiry".to_string(),
            ExecutionLatency::Governance => {
                "no expiry, but the gas coin references go stale if any other \
                 transaction touches those coins before execution - keep this \
                 derived address quiet while the DAO votes"
                    .to_string()
            }
        };

        let display = format!(
            "\n\
             Unsigned {chain_key} transaction (NEAR {near_network}):\n\
             ------------------------------------------------------------\n\
             action:            transfer {amount} to {to_hex}\n\
             sender (from):     {sender_hex} (derived: {owner} / \"{derivation_path}\")\n\
             balance:           {balance}\n\
             gas:               budget {budget} at price {gas_price} mist/unit \
             ({gas_coins} gas coin(s); unused budget is refunded)\n\
             validity:          {validity_note}\n\
             tx digest:         {digest}\n\
             signing payload:   32-byte Blake2b-256 intent digest (signed as-is by the MPC)\n\
             ------------------------------------------------------------",
            chain_key = chain.chain_key,
            near_network = chain.near_network,
            amount = format_native(amount_mist, chain),
            to_hex = SuiAddress(to).to_hex(),
            balance = format_native(total_balance, chain),
            budget = format_native(GAS_BUDGET_MIST, chain),
            gas_coins = tx.gas_data.payment.len(),
            digest = tx.digest_base58(),
        );

        let mut unsigned_tx = serde_json::to_value(SuiUnsignedPayload {
            tx,
            sender_public_key: hex::encode(pk),
        })?;
        prettify_pure_call_args(&mut unsigned_tx);

        Ok(BuiltTransaction {
            unsigned_tx,
            payloads: vec![signing_digest],
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
    unprettify_pure_call_args(&mut unsigned_tx)?;
    let payload: SuiUnsignedPayload = serde_json::from_value(unsigned_tx)
        .wrap_err("Failed to deserialize the unsigned Sui transaction")?;
    Ok(vec![payload.tx.build_for_signing()])
}

/// Combines the unsigned Sui transaction with the MPC signature and
/// broadcasts it; returns the transaction digest.
pub fn assemble_and_broadcast(
    chain: &ResolvedChain,
    unsigned_tx: &serde_json::Value,
    signatures: &[MpcSignatureResponse],
) -> color_eyre::eyre::Result<String> {
    use base64::Engine;

    let mut unsigned_tx = unsigned_tx.clone();
    unprettify_pure_call_args(&mut unsigned_tx)?;
    let payload: SuiUnsignedPayload = serde_json::from_value(unsigned_tx)
        .wrap_err("Failed to deserialize the unsigned Sui transaction")?;
    let response = signatures
        .first()
        .wrap_err("No MPC signature available to assemble")?;
    let signature = ed25519_signature_from_mpc(response, &payload.sender_public_key)?;

    let engine = base64::engine::general_purpose::STANDARD;
    rpc::execute_transaction(
        &chain.rpc_url,
        &engine.encode(payload.tx.tx_bytes()),
        &engine.encode(signature.to_bytes()),
    )
}

/// Envelope prettification: `Pure` call args are BCS byte blobs that the
/// upstream serde emits as number arrays; show them as hex (a transfer's
/// second input is the recipient address).
fn prettify_pure_call_args(unsigned_tx: &mut serde_json::Value) {
    if let Some(inputs) = unsigned_tx
        .pointer_mut("/tx/kind/ProgrammableTransaction/inputs")
        .and_then(serde_json::Value::as_array_mut)
    {
        for input in inputs {
            if let Some(pure) = input.get_mut("Pure") {
                crate::chains::bytes_array_to_hex(pure);
            }
        }
    }
}

fn unprettify_pure_call_args(unsigned_tx: &mut serde_json::Value) -> color_eyre::eyre::Result<()> {
    if let Some(inputs) = unsigned_tx
        .pointer_mut("/tx/kind/ProgrammableTransaction/inputs")
        .and_then(serde_json::Value::as_array_mut)
    {
        for input in inputs {
            if let Some(pure) = input.get_mut("Pure") {
                crate::chains::hex_to_bytes_array(pure)?;
            }
        }
    }
    Ok(())
}

pub fn ed25519_signature_from_mpc(
    response: &MpcSignatureResponse,
    sender_public_key_hex: &str,
) -> color_eyre::eyre::Result<SuiSignature> {
    let MpcSignatureResponse::Ed25519 { signature } = response else {
        return Err(eyre!(
            "Expected an ed25519 MPC signature for a Sui chain, got a secp256k1 one"
        ));
    };
    let signature_bytes: [u8; 64] = signature.as_slice().try_into().map_err(|_| {
        eyre!(
            "MPC ed25519 signature must be 64 bytes, got {}",
            signature.len()
        )
    })?;
    let mut public_key = [0u8; 32];
    hex::decode_to_slice(sender_public_key_hex, &mut public_key)
        .wrap_err("Invalid sender public key in the envelope")?;
    Ok(SuiSignature::ed25519(signature_bytes, public_key))
}

fn format_native(mist: u64, chain: &ResolvedChain) -> String {
    crate::types::format_move_style_amount(
        mist,
        &chain.symbol,
        "mist",
        10u64.pow(u32::from(chain.decimals)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey, Verifier};

    /// Round-trip through a real ed25519 key: build a transfer PTB, sign the
    /// Blake2b intent digest the way the MPC does, and verify the Sui
    /// signature envelope layout (`0x00 || sig || pk`) plus digest math.
    #[test]
    fn ed25519_signature_assembles_and_verifies() {
        let signing_key = SigningKey::from_bytes(&[21u8; 32]);
        let pk = signing_key.verifying_key().to_bytes();
        let sender = derive_sui_address(SuiSignatureScheme::Ed25519, &pk);

        let tx = SuiTransaction {
            kind: TransactionKind::ProgrammableTransaction(ProgrammableTransaction {
                inputs: vec![
                    CallArg::pure_u64(1_000_000),
                    CallArg::pure_address(SuiAddress([9u8; 32])),
                ],
                commands: vec![
                    Command::SplitCoins {
                        coin: Argument::GasCoin,
                        amounts: vec![Argument::Input(0)],
                    },
                    Command::TransferObjects {
                        objects: vec![Argument::Result(0)],
                        address: Argument::Input(1),
                    },
                ],
            }),
            sender,
            gas_data: GasData {
                payment: vec![ObjectRef::new(
                    SuiAddress([1u8; 32]),
                    2,
                    ObjectDigest::new([0x63u8; 32]),
                )],
                owner: sender,
                price: 1000,
                budget: GAS_BUDGET_MIST,
            },
            expiration: TransactionExpiration::None,
        };

        // The signing payload is the Blake2b-256 digest of the intent message
        let digest = tx.build_for_signing();
        assert_eq!(digest.len(), 32);
        assert_eq!(
            digest,
            omni_transaction::sui::utils::blake2b256(&tx.build_intent_message()).to_vec()
        );

        // Sui validators verify ed25519 over the digest itself
        let signature = signing_key.sign(&digest);
        signing_key
            .verifying_key()
            .verify(&digest, &signature)
            .unwrap();

        let response = MpcSignatureResponse::Ed25519 {
            signature: signature.to_bytes().to_vec(),
        };
        let sui_signature = ed25519_signature_from_mpc(&response, &hex::encode(pk)).unwrap();
        let bytes = sui_signature.to_bytes();
        assert_eq!(bytes.len(), 1 + 64 + 32);
        assert_eq!(bytes[0], 0x00); // ed25519 flag
        assert_eq!(&bytes[1..65], signature.to_bytes().as_slice());
        assert_eq!(&bytes[65..], pk.as_slice());
    }

    /// The envelope form shows pure call args as hex and round-trips exactly.
    #[test]
    fn envelope_pure_args_prettify_and_round_trip() {
        let sender = SuiAddress([1u8; 32]);
        let tx = SuiTransaction {
            kind: TransactionKind::ProgrammableTransaction(ProgrammableTransaction {
                inputs: vec![
                    CallArg::pure_u64(1_000_000),
                    CallArg::pure_address(SuiAddress([9u8; 32])),
                ],
                commands: vec![Command::SplitCoins {
                    coin: Argument::GasCoin,
                    amounts: vec![Argument::Input(0)],
                }],
            }),
            sender,
            gas_data: GasData {
                payment: vec![ObjectRef::new(
                    SuiAddress([2u8; 32]),
                    3,
                    ObjectDigest::new([0x63u8; 32]),
                )],
                owner: sender,
                price: 1000,
                budget: GAS_BUDGET_MIST,
            },
            expiration: TransactionExpiration::None,
        };
        let expected_digest = tx.build_for_signing();

        let mut unsigned_tx = serde_json::to_value(SuiUnsignedPayload {
            tx,
            sender_public_key: "00".repeat(32),
        })
        .unwrap();
        prettify_pure_call_args(&mut unsigned_tx);
        let inputs = &unsigned_tx["tx"]["kind"]["ProgrammableTransaction"]["inputs"];
        assert_eq!(inputs[1]["Pure"], format!("0x{}", "09".repeat(32)));

        unprettify_pure_call_args(&mut unsigned_tx).unwrap();
        let restored: SuiUnsignedPayload = serde_json::from_value(unsigned_tx).unwrap();
        assert_eq!(restored.tx.build_for_signing(), expected_digest);
    }
}
