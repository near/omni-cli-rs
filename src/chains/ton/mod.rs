//! TON chain family: v5r1 (W5) wallet transactions built with
//! omni-transaction-rs TON builders. The MPC signs the 32-byte cell hash of
//! the wallet body verbatim via its ed25519 key domain. The wallet contract
//! is deployed automatically with the first transaction (`StateInit`
//! attached when the wallet is not active yet).

pub mod rpc;

use color_eyre::eyre::{ContextCompat, WrapErr, eyre};
use color_eyre::owo_colors::OwoColorize;
use omni_transaction::ton::TonTransaction;
use omni_transaction::ton::types::{
    Coins, InternalMessage, MAINNET_GLOBAL_ID, TESTNET_GLOBAL_ID, TonAddress, WalletVersion,
    parse_boc_single_root, v5r1_wallet_id,
};
use omni_transaction::ton::utils::derive_wallet_address;

use crate::chains::{BuiltTransaction, ChainAdapter, ExecutionLatency, SignatureScheme};
use crate::config::ResolvedChain;
use crate::mpc::MpcSignatureResponse;

pub const FAMILY: &str = "ton";

const WORKCHAIN: i8 = 0;
const IMMEDIATE_VALIDITY_SECS: u64 = 10 * 60;
const GOVERNANCE_VALIDITY_SECS: u64 = 14 * 24 * 60 * 60;

#[derive(Debug, Clone)]
pub enum TonActionSpec {
    /// Native TON transfer. Sent non-bounceable so not-yet-deployed
    /// recipient wallets keep the funds (standard wallet behavior).
    Transfer { to: TonAddress, nanotons: u64 },
    /// An internal message with a body - a contract call (jetton transfer,
    /// NFT, DEX, ...) or a text comment - and an explicit bounce flag.
    Message {
        to: TonAddress,
        nanotons: u64,
        body: TonBody,
        bounce: bool,
    },
}

/// The body of a TON internal message.
#[derive(Debug, Clone)]
pub enum TonBody {
    None,
    /// A text comment (op 0 + UTF-8), the convention wallets display.
    Comment(String),
    /// A pre-built body cell as a BOC (e.g. from a dApp or `tonutils`).
    Boc(Vec<u8>),
}

pub struct TonAdapter {
    pub spec: TonActionSpec,
}

/// v5r1 wallet ids are network-scoped (they encode the network global id).
fn wallet_id(chain: &ResolvedChain) -> u32 {
    let global_id = if chain.near_network == "mainnet" {
        MAINNET_GLOBAL_ID
    } else {
        TESTNET_GLOBAL_ID
    };
    v5r1_wallet_id(global_id, WORKCHAIN, 0)
}

pub(crate) fn wallet_address_string(public_key: &[u8; 32], chain: &ResolvedChain) -> String {
    derive_wallet_address(WalletVersion::V5R1, WORKCHAIN, wallet_id(chain), public_key)
        // Non-bounceable form (UQ...), the standard display for wallets.
        .to_base64_string(false, chain.near_network != "mainnet")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after 1970")
        .as_secs()
}

impl ChainAdapter for TonAdapter {
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
        // The address depends on the network (wallet id); show the mainnet
        // form here, build() prints the network-correct one.
        let pk = crate::mpc::ed25519_bytes(public_key)?;
        Ok(derive_wallet_address(
            WalletVersion::V5R1,
            WORKCHAIN,
            v5r1_wallet_id(MAINNET_GLOBAL_ID, WORKCHAIN, 0),
            &pk,
        )
        .to_base64_string(false, false))
    }

    fn build(
        &self,
        chain: &ResolvedChain,
        derived_public_key: &near_crypto::PublicKey,
        owner: &str,
        derivation_path: &str,
        latency: ExecutionLatency,
    ) -> color_eyre::eyre::Result<BuiltTransaction> {
        let public_key = crate::mpc::ed25519_bytes(derived_public_key)?;
        let wallet_id = wallet_id(chain);
        let wallet_address = wallet_address_string(&public_key, chain);

        let info = rpc::Client::new(&chain.rpc_url)?
            .wallet_information(&wallet_address)
            .wrap_err_with(|| format!("Failed to fetch wallet state from {}", chain.rpc_url))?;

        let (to, nanotons, message, action) = match &self.spec {
            TonActionSpec::Transfer { to, nanotons } => {
                let mut message =
                    InternalMessage::new(*to, Coins::from_nano(u128::from(*nanotons)));
                // Non-bounceable: funds stay with not-yet-deployed recipient wallets.
                message.bounce = false;
                (to, *nanotons, message, "transfer".to_string())
            }
            TonActionSpec::Message {
                to,
                nanotons,
                body,
                bounce,
            } => {
                let mut message =
                    InternalMessage::new(*to, Coins::from_nano(u128::from(*nanotons)));
                message.bounce = *bounce;
                let action = match body {
                    TonBody::None => "send".to_string(),
                    TonBody::Comment(text) => {
                        message = message
                            .with_comment(text)
                            .map_err(|err| eyre!("Comment does not fit a cell: {err:?}"))?;
                        format!("send with comment {text:?}")
                    }
                    TonBody::Boc(bytes) => {
                        message.body = Some(
                            parse_boc_single_root(bytes)
                                .map_err(|err| eyre!("Invalid body BOC: {err:?}"))?,
                        );
                        format!("send with a {}-byte body BOC", bytes.len())
                    }
                };
                (
                    to,
                    *nanotons,
                    message,
                    format!(
                        "{action} ({})",
                        if *bounce {
                            "bounceable"
                        } else {
                            "non-bounceable"
                        }
                    ),
                )
            }
        };
        // Non-bounceable, network-correct rendering for the summary.
        let to_display = to.to_base64_string(false, chain.near_network != "mainnet");

        let (validity_secs, validity_note) = match latency {
            ExecutionLatency::Immediate => {
                (IMMEDIATE_VALIDITY_SECS, "expires in 10 minutes".to_string())
            }
            ExecutionLatency::Governance => (
                GOVERNANCE_VALIDITY_SECS,
                format!(
                    "expires in 14 days (DAO voting window); assumes seqno {} is still \
                     current at execution time",
                    info.seqno
                ),
            ),
        };
        let valid_until = (unix_now() + validity_secs)
            .try_into()
            .wrap_err("valid_until overflows u32")?;

        let tx = TonTransaction {
            wallet_version: WalletVersion::V5R1,
            workchain: WORKCHAIN,
            public_key,
            wallet_id,
            valid_until,
            seqno: info.seqno,
            messages: vec![message],
            deploy: !info.deployed,
        };
        let payload = tx.build_for_signing();

        let required = nanotons + 10_000_000; // ~0.01 TON headroom for fees
        let balance_note = if info.balance_nanotons < required {
            format!(
                "\n   {warning} balance {} is below the required {} (amount + fees) - \
                 fund the derived wallet first",
                format_native(info.balance_nanotons, chain),
                format_native(required, chain),
                warning = "WARNING:".yellow(),
            )
        } else {
            String::new()
        };

        let display = format!(
            "\n\
             Unsigned {chain_key} transaction (NEAR {near_network}):\n\
             ------------------------------------------------------------\n\
             action:            {action} {amount} to {to_display}\n\
             wallet (from):     {wallet_address} (v5r1, derived: {owner} / \"{derivation_path}\")\n\
             balance:           {balance}{balance_note}\n\
             seqno:             {seqno}{deploy_note}\n\
             valid until:       unix {valid_until} ({validity_note})\n\
             signing payload:   32-byte wallet-body cell hash (signed as-is by the MPC)\n\
             ------------------------------------------------------------",
            chain_key = chain.chain_key,
            near_network = chain.near_network,
            amount = format_native(nanotons, chain),
            balance = format_native(info.balance_nanotons, chain),
            seqno = info.seqno,
            deploy_note = if info.deployed {
                ""
            } else {
                " (wallet not deployed yet - StateInit attached, deployed with this transaction)"
            },
        );

        Ok(BuiltTransaction {
            unsigned_tx: serde_json::to_value(&tx)?,
            payloads: vec![payload],
            display,
            after_broadcast: None,
        })
    }
}

/// Recomputes the MPC signing payload from an envelope's unsigned tx -
/// the byte-equality half of `proposal review`.
pub(crate) fn signing_payloads_from_envelope(
    unsigned_tx: &serde_json::Value,
) -> color_eyre::eyre::Result<Vec<Vec<u8>>> {
    let tx: TonTransaction = serde_json::from_value(unsigned_tx.clone())
        .wrap_err("Failed to deserialize the unsigned TON transaction")?;
    Ok(vec![tx.build_for_signing()])
}

/// Combines the unsigned TON transaction with the MPC signature and
/// broadcasts it; returns the external message hash (hex).
pub fn assemble_and_broadcast(
    chain: &ResolvedChain,
    unsigned_tx: &serde_json::Value,
    signatures: &[MpcSignatureResponse],
) -> color_eyre::eyre::Result<String> {
    let tx: TonTransaction = serde_json::from_value(unsigned_tx.clone())
        .wrap_err("Failed to deserialize the unsigned TON transaction")?;
    let response = signatures
        .first()
        .wrap_err("No MPC signature available to assemble")?;
    let MpcSignatureResponse::Ed25519 { signature } = response else {
        return Err(eyre!(
            "Expected an ed25519 MPC signature for a TON chain, got a secp256k1 one"
        ));
    };
    let signature: [u8; 64] = signature.as_slice().try_into().map_err(|_| {
        eyre!(
            "MPC ed25519 signature must be 64 bytes, got {}",
            signature.len()
        )
    })?;
    let boc = tx.build_with_signature(signature);
    rpc::Client::new(&chain.rpc_url)?.send_boc(&boc)
}

fn format_native(nanotons: u64, chain: &ResolvedChain) -> String {
    crate::types::format_move_style_amount(
        nanotons,
        &chain.symbol,
        "nanotons",
        10u64.pow(u32::from(chain.decimals)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey, Verifier};

    /// Round-trip through a real ed25519 key: build a v5r1 transfer, sign
    /// the cell-hash payload the way the MPC does, and confirm the signed
    /// BoC assembles and starts with the BoC magic.
    #[test]
    fn ed25519_signature_assembles_into_boc() {
        let signing_key = SigningKey::from_bytes(&[31u8; 32]);
        let public_key = signing_key.verifying_key().to_bytes();
        let dest: omni_transaction::ton::types::TonAddress =
            "EQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqB2N"
                .parse()
                .unwrap();

        let mut message = InternalMessage::new(dest, Coins::from_nano(50_000_000));
        message.bounce = false;
        let tx = TonTransaction {
            wallet_version: WalletVersion::V5R1,
            workchain: WORKCHAIN,
            public_key,
            wallet_id: v5r1_wallet_id(MAINNET_GLOBAL_ID, WORKCHAIN, 0),
            valid_until: 1_800_000_000,
            seqno: 0,
            messages: vec![message],
            deploy: true,
        };

        let payload = tx.build_for_signing();
        assert_eq!(payload.len(), 32);

        let signature = signing_key.sign(&payload);
        signing_key
            .verifying_key()
            .verify(&payload, &signature)
            .unwrap();

        let boc = tx.build_with_signature(signature.to_bytes());
        // BoC magic: b5ee9c72
        assert_eq!(&boc[..4], &[0xb5, 0xee, 0x9c, 0x72]);

        // Envelope round trip preserves the signing payload
        let json = serde_json::to_value(&tx).unwrap();
        let restored: TonTransaction = serde_json::from_value(json).unwrap();
        assert_eq!(restored.build_for_signing(), payload);
    }

    #[test]
    fn wallet_ids_are_network_scoped() {
        assert_eq!(v5r1_wallet_id(MAINNET_GLOBAL_ID, 0, 0), 0x7FFF_FF11);
        assert_ne!(
            v5r1_wallet_id(MAINNET_GLOBAL_ID, 0, 0),
            v5r1_wallet_id(TESTNET_GLOBAL_ID, 0, 0)
        );
    }
}
