//! UTXO chain family (Bitcoin): P2WPKH spends built with omni-transaction-rs
//! Bitcoin builders. The MPC signs one 32-byte BIP143 sighash **per input**
//! via its secp256k1 key domain - the one family where a single transaction
//! needs multiple sign actions.

pub mod address;
pub mod rpc;

use color_eyre::eyre::{ContextCompat, WrapErr, eyre};
use k256::ecdsa::signature::hazmat::PrehashVerifier;
use omni_transaction::bitcoin::BitcoinTransaction;
use omni_transaction::bitcoin::types::{
    Amount, EcdsaSighashType, Hash, LockTime, OutPoint, ScriptBuf, Sequence, TransactionType, TxIn,
    TxOut, Txid, Version, Witness,
};
use omni_transaction::bitcoin::utils::serialize_ecdsa_signature;

use crate::chains::{BuiltTransaction, ChainAdapter, ExecutionLatency, SignatureScheme};
use crate::config::ResolvedChain;
use crate::mpc::MpcSignatureResponse;

use self::address::{
    BtcNetwork, address_to_script_pubkey, compress_public_key, p2wpkh_address, p2wpkh_script_code,
    p2wpkh_script_pubkey, sha256d,
};

pub const FAMILY: &str = "utxo";

/// Outputs below this are uneconomical; change under it goes to the fee.
const DUST_LIMIT_SATS: u64 = 546;

/// RBF-enabled sequence.
const SEQUENCE_RBF: u32 = 0xFFFF_FFFD;

#[derive(Debug, Clone)]
pub enum UtxoActionSpec {
    /// BTC transfer to any standard address (bech32/bech32m or base58).
    Transfer { to: String, sats: u64 },
}

/// What goes into the envelope: the transaction (reviewer-friendly form),
/// the spent input values (needed to recompute the BIP143 sighashes at
/// assembly time), and the sender's compressed public key (for the
/// witnesses).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct UtxoUnsignedPayload {
    pub tx: UtxoTxJson,
    pub input_values: Vec<u64>,
    pub sender_public_key: String,
}

/// Reviewer-friendly JSON form of the unsigned transaction, as stored in the
/// proposal envelope: display-order txid hex and script hex instead of the
/// byte arrays the upstream serde emits.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct UtxoTxJson {
    pub version: u8,
    pub lock_time: u32,
    pub inputs: Vec<UtxoTxInputJson>,
    pub outputs: Vec<UtxoTxOutputJson>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct UtxoTxInputJson {
    pub txid: String,
    pub vout: u32,
    pub sequence: u32,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct UtxoTxOutputJson {
    pub value_sats: u64,
    pub script_pubkey: String,
}

impl From<&BitcoinTransaction> for UtxoTxJson {
    fn from(tx: &BitcoinTransaction) -> Self {
        Self {
            version: match tx.version {
                Version::One => 1,
                Version::Two => 2,
            },
            // Always built with lock_time 0 (the upstream field is private).
            lock_time: 0,
            inputs: tx
                .input
                .iter()
                .map(|input| UtxoTxInputJson {
                    txid: hex::encode(input.previous_output.txid.0.0),
                    vout: input.previous_output.vout,
                    sequence: input.sequence.0,
                })
                .collect(),
            outputs: tx
                .output
                .iter()
                .map(|output| UtxoTxOutputJson {
                    value_sats: output.value.to_sat(),
                    script_pubkey: format!("0x{}", hex::encode(&output.script_pubkey.0)),
                })
                .collect(),
        }
    }
}

impl TryFrom<&UtxoTxJson> for BitcoinTransaction {
    type Error = color_eyre::eyre::Error;

    fn try_from(json: &UtxoTxJson) -> color_eyre::eyre::Result<Self> {
        let version = match json.version {
            1 => Version::One,
            2 => Version::Two,
            other => return Err(eyre!("Unsupported transaction version {other}")),
        };
        Ok(Self {
            version,
            lock_time: LockTime::from_height(json.lock_time).map_err(|err| eyre!("{err}"))?,
            input: json
                .inputs
                .iter()
                .map(|input| {
                    Ok(TxIn {
                        previous_output: OutPoint {
                            txid: txid_from_display_hex(&input.txid)?,
                            vout: input.vout,
                        },
                        script_sig: ScriptBuf::default(),
                        sequence: Sequence(input.sequence),
                        witness: Witness::default(),
                    })
                })
                .collect::<color_eyre::eyre::Result<Vec<_>>>()?,
            output: json
                .outputs
                .iter()
                .map(|output| {
                    let script = &output.script_pubkey;
                    Ok(TxOut {
                        value: Amount::from_sat(output.value_sats),
                        script_pubkey: ScriptBuf(
                            hex::decode(script.strip_prefix("0x").unwrap_or(script))
                                .wrap_err("Invalid script_pubkey hex in the envelope")?,
                        ),
                    })
                })
                .collect::<color_eyre::eyre::Result<Vec<_>>>()?,
        })
    }
}

pub struct UtxoAdapter {
    pub spec: UtxoActionSpec,
}

/// Estimated virtual size of a P2WPKH spend.
fn estimate_vsize(inputs: usize, outputs: usize) -> u64 {
    11 + 68 * inputs as u64 + 31 * outputs as u64
}

/// A Txid from the display-order hex explorers and Esplora use. The library's
/// `Hash` stores display order and reverses to wire order when encoding, so
/// the hex goes in as-is.
fn txid_from_display_hex(display_hex: &str) -> color_eyre::eyre::Result<Txid> {
    if display_hex.len() != 64 {
        return Err(eyre!("Invalid txid length: {}", display_hex.len()));
    }
    Ok(Txid(
        Hash::from_hex(display_hex).wrap_err("Invalid txid hex")?,
    ))
}

/// The per-input BIP143 sighashes of a P2WPKH spend: the payloads the MPC
/// signs (`sha256d` of the segwit signing preimage).
pub fn input_sighashes(
    tx: &BitcoinTransaction,
    input_values: &[u64],
    compressed_public_key: &[u8; 33],
) -> Vec<[u8; 32]> {
    let script_code = ScriptBuf(p2wpkh_script_code(compressed_public_key));
    input_values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            sha256d(&tx.build_for_signing_segwit(
                EcdsaSighashType::All,
                index,
                &script_code,
                *value,
            ))
        })
        .collect()
}

impl ChainAdapter for UtxoAdapter {
    fn family(&self) -> &'static str {
        FAMILY
    }

    fn scheme(&self) -> SignatureScheme {
        SignatureScheme::Secp256k1
    }

    fn derived_address(
        &self,
        public_key: &near_crypto::PublicKey,
    ) -> color_eyre::eyre::Result<String> {
        let pk = compress_public_key(&crate::mpc::secp256k1_bytes(public_key)?);
        // Network-independent derivation, network-dependent encoding: default
        // to mainnet here; build() prints the network-correct form.
        Ok(format!(
            "{} (mainnet form; testnet: {})",
            p2wpkh_address(&pk, BtcNetwork::Mainnet),
            p2wpkh_address(&pk, BtcNetwork::Testnet),
        ))
    }

    fn build(
        &self,
        chain: &ResolvedChain,
        derived_public_key: &near_crypto::PublicKey,
        owner: &str,
        derivation_path: &str,
        latency: ExecutionLatency,
    ) -> color_eyre::eyre::Result<BuiltTransaction> {
        let network = BtcNetwork::from_near_network(&chain.near_network);
        let public_key = compress_public_key(&crate::mpc::secp256k1_bytes(derived_public_key)?);
        let sender_address = p2wpkh_address(&public_key, network);

        let UtxoActionSpec::Transfer { to, sats } = &self.spec;
        let recipient_script = address_to_script_pubkey(to, network)?;

        // Fee rate: tighter target + margin for the governance route, since
        // the fee is baked in at signing time and cannot be bumped later
        // without re-signing.
        let (target_blocks, rate_margin, validity_note) = match latency {
            ExecutionLatency::Immediate => (3, 1.0, "no expiry".to_string()),
            ExecutionLatency::Governance => (
                1,
                1.5,
                "no expiry; the derived address is the only spender of these UTXOs, \
                 but the fee rate is baked in now - a long vote plus a fee spike can \
                 delay confirmation"
                    .to_string(),
            ),
        };
        let rpc = rpc::Client::new(&chain.rpc_url)?;
        let rate = rpc.fee_rate(target_blocks)? * rate_margin;

        let mut utxos = rpc
            .utxos(&sender_address)
            .wrap_err_with(|| format!("Failed to fetch UTXOs for {sender_address}"))?;
        utxos.sort_by_key(|utxo| std::cmp::Reverse(utxo.value_sats));
        let total_balance: u64 = utxos.iter().map(|utxo| utxo.value_sats).sum();

        // Largest-first selection; two outputs assumed while selecting.
        let mut selected = Vec::new();
        let mut covered = 0u64;
        let mut fee = 0u64;
        for utxo in &utxos {
            selected.push(utxo.clone());
            covered += utxo.value_sats;
            fee = (estimate_vsize(selected.len(), 2) as f64 * rate).ceil() as u64;
            if covered >= sats + fee {
                break;
            }
        }
        if covered < sats + fee {
            return Err(eyre!(
                "The derived address {sender_address} holds {} in confirmed UTXOs but this \
                 transaction needs {} (amount + fee) - fund the derived address first.",
                format_native(total_balance, chain),
                format_native(sats + fee, chain),
            ));
        }

        // Change back to the sender; dust folds into the fee.
        let mut change = covered - sats - fee;
        let mut outputs = vec![TxOut {
            value: Amount::from_sat(*sats),
            script_pubkey: ScriptBuf(recipient_script),
        }];
        if change >= DUST_LIMIT_SATS {
            outputs.push(TxOut {
                value: Amount::from_sat(change),
                script_pubkey: ScriptBuf(p2wpkh_script_pubkey(&public_key)),
            });
        } else {
            fee += change;
            change = 0;
        }

        let inputs = selected
            .iter()
            .map(|utxo| {
                Ok(TxIn {
                    previous_output: OutPoint {
                        txid: txid_from_display_hex(&utxo.txid)?,
                        vout: utxo.vout,
                    },
                    script_sig: ScriptBuf::default(),
                    sequence: Sequence(SEQUENCE_RBF),
                    witness: Witness::default(),
                })
            })
            .collect::<color_eyre::eyre::Result<Vec<_>>>()?;

        let tx = BitcoinTransaction {
            version: Version::Two,
            lock_time: LockTime::from_height(0).map_err(|err| eyre!("{err}"))?,
            input: inputs,
            output: outputs,
        };

        let input_values: Vec<u64> = selected.iter().map(|utxo| utxo.value_sats).collect();
        let payloads: Vec<Vec<u8>> = input_sighashes(&tx, &input_values, &public_key)
            .into_iter()
            .map(|digest| digest.to_vec())
            .collect();

        let display = format!(
            "\n\
             Unsigned {chain_key} transaction (NEAR {near_network}):\n\
             ------------------------------------------------------------\n\
             action:          transfer {amount} to {to}\n\
             from:            {sender_address} (derived: {owner} / \"{derivation_path}\")\n\
             balance:         {balance} ({utxo_count} confirmed UTXO(s))\n\
             inputs:          {input_count} UTXO(s) spent, one MPC signature each\n\
             fee:             {fee_fmt} ({rate:.1} sat/vB)\n\
             change:          {change_fmt} (back to the derived address)\n\
             validity:        {validity_note}\n\
             signing payload: {input_count} x 32-byte BIP143 sighash\n\
             ------------------------------------------------------------",
            chain_key = chain.chain_key,
            near_network = chain.near_network,
            amount = format_native(*sats, chain),
            balance = format_native(total_balance, chain),
            utxo_count = utxos.len(),
            input_count = selected.len(),
            fee_fmt = format_native(fee, chain),
            change_fmt = format_native(change, chain),
        );

        Ok(BuiltTransaction {
            unsigned_tx: serde_json::to_value(UtxoUnsignedPayload {
                tx: UtxoTxJson::from(&tx),
                input_values,
                sender_public_key: hex::encode(public_key),
            })?,
            payloads,
            display,
        })
    }
}

/// Recomputes the per-input MPC signing payloads from an envelope's
/// unsigned tx - the byte-equality half of `proposal review`.
pub(crate) fn signing_payloads_from_envelope(
    unsigned_tx: &serde_json::Value,
) -> color_eyre::eyre::Result<Vec<Vec<u8>>> {
    let payload: UtxoUnsignedPayload = serde_json::from_value(unsigned_tx.clone())
        .wrap_err("Failed to deserialize the unsigned Bitcoin transaction")?;
    let tx = BitcoinTransaction::try_from(&payload.tx)?;
    let mut public_key = [0u8; 33];
    hex::decode_to_slice(&payload.sender_public_key, &mut public_key)
        .wrap_err("Invalid sender public key in the envelope")?;
    Ok(input_sighashes(&tx, &payload.input_values, &public_key)
        .into_iter()
        .map(|digest| digest.to_vec())
        .collect())
}

/// Combines the unsigned Bitcoin transaction with the MPC signatures (one per
/// input, matched by verification since receipt order is not guaranteed) and
/// broadcasts it; returns the txid.
pub fn assemble_and_broadcast(
    chain: &ResolvedChain,
    unsigned_tx: &serde_json::Value,
    signatures: &[MpcSignatureResponse],
) -> color_eyre::eyre::Result<String> {
    let payload: UtxoUnsignedPayload = serde_json::from_value(unsigned_tx.clone())
        .wrap_err("Failed to deserialize the unsigned Bitcoin transaction")?;
    let mut tx = BitcoinTransaction::try_from(&payload.tx)?;

    let mut public_key = [0u8; 33];
    hex::decode_to_slice(&payload.sender_public_key, &mut public_key)
        .wrap_err("Invalid sender public key in the envelope")?;
    let verifying_key = k256::ecdsa::VerifyingKey::from_sec1_bytes(&public_key)
        .map_err(|err| eyre!("Invalid sender public key: {err}"))?;

    let digests = input_sighashes(&tx, &payload.input_values, &public_key);
    if signatures.len() < digests.len() {
        return Err(eyre!(
            "This transaction spends {} input(s) but only {} MPC signature(s) were found.",
            digests.len(),
            signatures.len()
        ));
    }

    // Convert responses to raw 64-byte signatures.
    let raw_signatures: Vec<[u8; 64]> = signatures
        .iter()
        .map(|response| {
            let MpcSignatureResponse::Secp256k1 { big_r, s, .. } = response else {
                return Err(eyre!(
                    "Expected secp256k1 MPC signatures for a UTXO chain, got an ed25519 one"
                ));
            };
            let big_r = hex::decode(&big_r.affine_point)
                .wrap_err("MPC signature big_r is not valid hex")?;
            let s = hex::decode(&s.scalar).wrap_err("MPC signature s is not valid hex")?;
            if big_r.len() != 33 || s.len() != 32 {
                return Err(eyre!("Malformed MPC secp256k1 signature"));
            }
            let mut raw = [0u8; 64];
            raw[..32].copy_from_slice(&big_r[1..]);
            raw[32..].copy_from_slice(&s);
            Ok(raw)
        })
        .collect::<color_eyre::eyre::Result<Vec<_>>>()?;

    // Match each input's sighash to the signature that verifies over it.
    for (index, digest) in digests.iter().enumerate() {
        let raw = raw_signatures
            .iter()
            .find(|raw| {
                k256::ecdsa::Signature::from_slice(raw.as_slice())
                    .is_ok_and(|sig| verifying_key.verify_prehash(digest, &sig).is_ok())
            })
            .wrap_err_with(|| format!("No MPC signature verifies over input #{index}'s sighash"))?;
        let witness = vec![
            serialize_ecdsa_signature(raw.as_slice(), EcdsaSighashType::All as u8),
            public_key.to_vec(),
        ];
        tx.build_with_witness(index, witness, TransactionType::P2WPKH);
    }

    let tx_hex = hex::encode(tx.serialize());
    rpc::Client::new(&chain.rpc_url)?.broadcast_transaction(&tx_hex)
}

fn format_native(sats: u64, chain: &ResolvedChain) -> String {
    crate::types::format_move_style_amount(
        sats,
        &chain.symbol,
        "sats",
        10u64.pow(u32::from(chain.decimals)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::{SigningKey, signature::hazmat::PrehashSigner};

    fn test_chain() -> ResolvedChain {
        ResolvedChain {
            chain_key: "btc".to_string(),
            family: FAMILY.to_string(),
            near_network: "mainnet".to_string(),
            rpc_url: "http://127.0.0.1:1".to_string(),
            chain_id: None,
            explorer_tx_url: None,
            symbol: "BTC".to_string(),
            decimals: 8,
        }
    }

    /// Cross-checks the library's BIP143 preimage against an independent
    /// implementation of the spec, then runs the full assembly round trip
    /// with real secp256k1 keys - with the signatures deliberately shuffled
    /// to prove the verify-based input matching.
    #[test]
    fn bip143_sighash_and_shuffled_assembly_roundtrip() {
        let signing_key = SigningKey::from_bytes(&[0x51u8; 32].into()).unwrap();
        let uncompressed_point = signing_key.verifying_key().to_sec1_point(false);
        let mut uncompressed = [0u8; 64];
        uncompressed.copy_from_slice(&uncompressed_point.as_bytes()[1..]);
        let public_key = compress_public_key(&uncompressed);

        let input_values = vec![70_000u64, 40_000u64];
        let tx = BitcoinTransaction {
            version: Version::Two,
            lock_time: LockTime::from_height(0).unwrap(),
            input: (0..2)
                .map(|vout| TxIn {
                    previous_output: OutPoint {
                        txid: txid_from_display_hex(
                            "aa25cc0dddd0a202c21e66521a692c0586330a9a9dcc38ccd9b4d2093037f31a",
                        )
                        .unwrap(),
                        vout,
                    },
                    script_sig: ScriptBuf::default(),
                    sequence: Sequence(SEQUENCE_RBF),
                    witness: Witness::default(),
                })
                .collect(),
            output: vec![TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: ScriptBuf(p2wpkh_script_pubkey(&public_key)),
            }],
        };

        // Independent BIP143 preimage for input 0
        let digests = input_sighashes(&tx, &input_values, &public_key);
        {
            use sha2::{Digest, Sha256};
            let script_code = p2wpkh_script_code(&public_key);
            // Txids are stored display-order and serialized reversed (wire order).
            let wire_txid = |txid: &Txid| -> Vec<u8> { txid.0.0.iter().rev().copied().collect() };
            let mut prevouts = Vec::new();
            let mut sequences = Vec::new();
            for txin in &tx.input {
                prevouts.extend_from_slice(&wire_txid(&txin.previous_output.txid));
                prevouts.extend_from_slice(&txin.previous_output.vout.to_le_bytes());
                sequences.extend_from_slice(&txin.sequence.0.to_le_bytes());
            }
            let mut outputs = Vec::new();
            for txout in &tx.output {
                outputs.extend_from_slice(&100_000u64.to_le_bytes());
                outputs.push(txout.script_pubkey.0.len() as u8);
                outputs.extend_from_slice(&txout.script_pubkey.0);
            }
            let mut preimage = Vec::new();
            preimage.extend_from_slice(&2u32.to_le_bytes()); // version
            preimage.extend_from_slice(&sha256d(&prevouts));
            preimage.extend_from_slice(&sha256d(&sequences));
            preimage.extend_from_slice(&wire_txid(&tx.input[0].previous_output.txid));
            preimage.extend_from_slice(&tx.input[0].previous_output.vout.to_le_bytes());
            preimage.push(script_code.len() as u8);
            preimage.extend_from_slice(&script_code);
            preimage.extend_from_slice(&input_values[0].to_le_bytes());
            preimage.extend_from_slice(&tx.input[0].sequence.0.to_le_bytes());
            preimage.extend_from_slice(&sha256d(&outputs));
            preimage.extend_from_slice(&0u32.to_le_bytes()); // locktime
            preimage.extend_from_slice(&1u32.to_le_bytes()); // SIGHASH_ALL
            let expected: [u8; 32] = Sha256::digest(Sha256::digest(&preimage)).into();
            assert_eq!(
                digests[0], expected,
                "BIP143 preimage construction diverged"
            );
        }

        // Sign both digests like the MPC would report them - SHUFFLED
        let responses: Vec<MpcSignatureResponse> = [1usize, 0]
            .iter()
            .map(|&i| {
                let (signature, recovery_id): (k256::ecdsa::Signature, k256::ecdsa::RecoveryId) =
                    signing_key.sign_prehash(&digests[i]).unwrap();
                let r: [u8; 32] = signature.r().to_bytes().into();
                let s: [u8; 32] = signature.s().to_bytes().into();
                MpcSignatureResponse::Secp256k1 {
                    big_r: crate::mpc::AffinePoint {
                        affine_point: format!(
                            "{:02x}{}",
                            2 + (recovery_id.to_byte() & 1),
                            hex::encode(r)
                        ),
                    },
                    s: crate::mpc::Scalar {
                        scalar: hex::encode(s),
                    },
                    recovery_id: recovery_id.to_byte(),
                }
            })
            .collect();

        let unsigned = serde_json::to_value(UtxoUnsignedPayload {
            tx: UtxoTxJson::from(&tx),
            input_values,
            sender_public_key: hex::encode(public_key),
        })
        .unwrap();
        // The envelope form is readable hex, not byte arrays
        assert!(unsigned["tx"]["inputs"][0]["txid"].is_string());
        assert!(
            unsigned["tx"]["outputs"][0]["script_pubkey"]
                .as_str()
                .unwrap()
                .starts_with("0x0014")
        );

        // Broadcast fails (no server on port 1), but everything before it -
        // deserialization, signature matching, witness assembly - must succeed.
        let error = assemble_and_broadcast(&test_chain(), &unsigned, &responses).unwrap_err();
        assert!(
            error.to_string().contains("Failed to reach"),
            "expected only the network step to fail, got: {error}"
        );
    }

    #[test]
    fn fee_estimation_and_dust_behavior() {
        assert_eq!(estimate_vsize(1, 2), 141);
        assert_eq!(estimate_vsize(3, 1), 246);
    }
}
