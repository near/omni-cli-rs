//! SVM chain family (Solana, Fogo, ...): ed25519 payloads built with
//! omni-transaction-rs Solana builders. The MPC signs the full serialized
//! message (never a hash) via its ed25519 key domain.

pub mod rpc;

use color_eyre::eyre::{ContextCompat, WrapErr, eyre};
use omni_transaction::TxBuilder;
use omni_transaction::solana::types::{Blockhash, SolanaAddress, SolanaSignature};
use omni_transaction::solana::{SolanaTransaction, SolanaTransactionBuilder, utils};

use crate::chains::{BuiltTransaction, ChainAdapter, ExecutionLatency, SignatureScheme};
use crate::config::ResolvedChain;
use crate::mpc::MpcSignatureResponse;

pub const FAMILY: &str = "svm";

/// Cost of one signature at the base fee rate.
const LAMPORTS_PER_SIGNATURE: u64 = 5_000;

#[derive(Debug, Clone)]
pub enum SvmActionSpec {
    /// Native SOL transfer via the system program.
    Transfer { to: String, lamports: u64 },
}

impl SvmActionSpec {
    fn summary(&self, chain: &ResolvedChain) -> String {
        match self {
            SvmActionSpec::Transfer { to, lamports } => {
                format!(
                    "transfer {} to {to}",
                    format_native(*lamports, chain)
                )
            }
        }
    }
}

pub struct SvmAdapter {
    pub spec: SvmActionSpec,
}

impl ChainAdapter for SvmAdapter {
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
        // A Solana address IS the ed25519 public key, base58-encoded.
        let bytes = crate::mpc::ed25519_bytes(public_key)?;
        Ok(SolanaAddress(bytes).to_base58())
    }

    fn build(
        &self,
        chain: &ResolvedChain,
        derived_public_key: &near_crypto::PublicKey,
        owner: &str,
        derivation_path: &str,
        latency: ExecutionLatency,
    ) -> color_eyre::eyre::Result<BuiltTransaction> {
        if latency == ExecutionLatency::Governance {
            return Err(eyre!(
                "sign-as-dao on SVM chains is not supported yet: a recent blockhash \
                 expires in ~60-90 seconds, long before a DAO can vote. It requires a \
                 durable nonce account per derived address (planned: \
                 `omni account setup-nonce`). Use sign-as-account for now."
            ));
        }

        let payer = SolanaAddress(crate::mpc::ed25519_bytes(derived_public_key)?);
        let payer_base58 = payer.to_base58();

        let instructions = match &self.spec {
            SvmActionSpec::Transfer { to, lamports } => {
                let to_address = SolanaAddress::from_base58(to)
                    .map_err(|err| eyre!("Invalid Solana address '{to}': {err}"))?;
                vec![utils::system_transfer(payer, to_address, *lamports)]
            }
        };

        let (recent_blockhash, last_valid_block_height) = rpc::latest_blockhash(&chain.rpc_url)
            .wrap_err_with(|| format!("Failed to fetch a recent blockhash from {}", chain.rpc_url))?;
        let blockhash = Blockhash::from_base58(&recent_blockhash)
            .map_err(|err| eyre!("Invalid blockhash from the RPC: {err}"))?;

        let tx = SolanaTransactionBuilder::new()
            .payer(payer)
            .instructions(instructions)
            .recent_blockhash(blockhash)
            .build();

        let payload = tx.build_for_signing();

        let balance = rpc::balance(&chain.rpc_url, &payer_base58).unwrap_or(0);
        let required = match &self.spec {
            SvmActionSpec::Transfer { lamports, .. } => lamports + LAMPORTS_PER_SIGNATURE,
        };
        let balance_note = if balance < required {
            format!(
                "\n   WARNING: balance {} is below the required {} (amount + fee) - fund the \
                 derived address first",
                format_native(balance, chain),
                format_native(required, chain),
            )
        } else {
            String::new()
        };

        let display = format!(
            "\n\
             Unsigned {chain_key} transaction (NEAR {near_network}):\n\
             ------------------------------------------------------------\n\
             action:            {summary}\n\
             fee payer (from):  {payer_base58} (derived: {owner} / \"{derivation_path}\")\n\
             balance:           {balance}{balance_note}\n\
             recent blockhash:  {recent_blockhash}\n\
             valid until:       block height {last_valid_block_height} (~60-90 seconds!)\n\
             base fee:          {fee}\n\
             signing payload:   {payload_len} bytes (full message, signed as-is by the MPC)\n\
             ------------------------------------------------------------",
            chain_key = chain.chain_key,
            near_network = chain.near_network,
            summary = self.spec.summary(chain),
            balance = format_native(balance, chain),
            fee = format_native(LAMPORTS_PER_SIGNATURE, chain),
            payload_len = payload.len(),
        );

        Ok(BuiltTransaction {
            unsigned_tx: serde_json::to_value(&tx)?,
            payloads: vec![payload],
            display,
        })
    }

}

/// Combines the unsigned Solana transaction with the MPC signature and
/// broadcasts it; returns the transaction signature (its id).
pub fn assemble_and_broadcast(
    chain: &ResolvedChain,
    unsigned_tx: &serde_json::Value,
    signatures: &[MpcSignatureResponse],
) -> color_eyre::eyre::Result<String> {
    let tx: SolanaTransaction = serde_json::from_value(unsigned_tx.clone())
        .wrap_err("Failed to deserialize the unsigned Solana transaction")?;
    let response = signatures
        .first()
        .wrap_err("No MPC signature available to assemble")?;
    let signature = signature_from_mpc(response)?;
    let wire_bytes = tx.build_with_signature(&[signature]);
    let tx_base64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&wire_bytes)
    };
    rpc::send_transaction(&chain.rpc_url, &tx_base64)
}

pub fn signature_from_mpc(
    response: &MpcSignatureResponse,
) -> color_eyre::eyre::Result<SolanaSignature> {
    let MpcSignatureResponse::Ed25519 { signature } = response else {
        return Err(eyre!(
            "Expected an ed25519 MPC signature for an SVM chain, got a secp256k1 one"
        ));
    };
    let bytes: [u8; 64] = signature.as_slice().try_into().map_err(|_| {
        eyre!(
            "MPC ed25519 signature must be 64 bytes, got {}",
            signature.len()
        )
    })?;
    Ok(SolanaSignature(bytes))
}

fn format_native(lamports: u64, chain: &ResolvedChain) -> String {
    let unit = 10u64.pow(chain.decimals as u32);
    if lamports == 0 {
        format!("0 {}", chain.symbol)
    } else if lamports.is_multiple_of(unit) {
        format!("{} {}", lamports / unit, chain.symbol)
    } else if lamports >= unit / 1_000_000 {
        format!(
            "{}.{} {}",
            lamports / unit,
            format!("{:0>width$}", lamports % unit, width = chain.decimals as usize)
                .trim_end_matches('0'),
            chain.symbol
        )
    } else {
        format!("{lamports} lamports")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_chain() -> ResolvedChain {
        ResolvedChain {
            chain_key: "solana".to_string(),
            family: FAMILY.to_string(),
            near_network: "testnet".to_string(),
            rpc_url: "http://127.0.0.1:1".to_string(),
            chain_id: None,
            explorer_tx_url: None,
            symbol: "SOL".to_string(),
            decimals: 9,
        }
    }

    /// Round-trip through a real ed25519 key: build a transfer, sign the full
    /// message payload the way the MPC does, assemble, and verify both the
    /// wire layout and the signature against the derived address.
    #[test]
    fn ed25519_signature_assembles_and_verifies() {
        use ed25519_dalek::{Signer, SigningKey, Verifier};

        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let payer_bytes = signing_key.verifying_key().to_bytes();
        let payer = SolanaAddress(payer_bytes);
        let to = SolanaAddress([9u8; 32]);

        let tx = SolanaTransactionBuilder::new()
            .payer(payer)
            .instructions(vec![utils::system_transfer(payer, to, 1_000_000)])
            .recent_blockhash(Blockhash([3u8; 32]))
            .build();
        let payload = tx.build_for_signing();

        // The MPC signs the full message bytes with ed25519 (no pre-hash)
        let signature = signing_key.sign(&payload);
        let response = MpcSignatureResponse::Ed25519 {
            signature: signature.to_bytes().to_vec(),
        };

        let solana_signature = signature_from_mpc(&response).unwrap();
        let wire_bytes = tx.build_with_signature(&[solana_signature]);

        // Wire layout: compact-u16 count (1), 64 signature bytes, then the message
        assert_eq!(wire_bytes[0], 1);
        assert_eq!(&wire_bytes[1..65], signature.to_bytes().as_slice());
        assert_eq!(&wire_bytes[65..], payload.as_slice());

        // And the signature must verify against the derived address (= pubkey)
        signing_key
            .verifying_key()
            .verify(&payload, &signature)
            .unwrap();

        // Adapter address derivation matches the payer
        let adapter = SvmAdapter {
            spec: SvmActionSpec::Transfer {
                to: to.to_base58(),
                lamports: 1_000_000,
            },
        };
        let near_pk = near_crypto::PublicKey::ED25519(near_crypto::ED25519PublicKey(payer_bytes));
        assert_eq!(adapter.derived_address(&near_pk).unwrap(), payer.to_base58());
    }

    #[test]
    fn governance_latency_is_rejected_until_durable_nonces() {
        let adapter = SvmAdapter {
            spec: SvmActionSpec::Transfer {
                to: SolanaAddress([9u8; 32]).to_base58(),
                lamports: 1,
            },
        };
        let near_pk = near_crypto::PublicKey::ED25519(near_crypto::ED25519PublicKey([7u8; 32]));
        let error = adapter
            .build(
                &test_chain(),
                &near_pk,
                "dao.near",
                "path",
                ExecutionLatency::Governance,
            )
            .unwrap_err();
        assert!(error.to_string().contains("durable nonce"));
    }
}
