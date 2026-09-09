pub mod aptos;
pub mod evm;
pub mod sui;
pub mod svm;
pub mod ton;
pub mod utxo;

use crate::config::ResolvedChain;

/// How long the gap between building a payload and executing it can be.
/// Chains punish that gap differently, so context fetching and validity
/// choices depend on it (e.g. fee ceilings on EVM, durable nonces on SVM).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionLatency {
    /// sign-as-account: seconds between build and broadcast
    Immediate,
    /// sign-as-dao: hours or days of voting before the signature exists
    Governance,
}

/// Which MPC key domain signs this chain's payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureScheme {
    Secp256k1,
    Ed25519,
}

/// Everything the executor needs after a family adapter built a transaction.
#[derive(Debug)]
pub struct BuiltTransaction {
    /// Family-specific serialization of the unsigned transaction - goes into
    /// the proposal envelope and the recovery command.
    pub unsigned_tx: serde_json::Value,
    /// MPC signing payloads, in the order the `sign` actions are emitted.
    /// One for most chains; one per input for UTXO chains.
    pub payloads: Vec<Vec<u8>>,
    /// Human-readable render shown before signing and in reviews.
    pub display: String,
}

/// One chain family. The construct flow builds an adapter from the action the
/// user described; everything downstream (derived-key lookup, MPC sign
/// request, envelope, signature assembly, broadcast) is family-agnostic and
/// dispatches through this trait.
pub trait ChainAdapter: Send + Sync {
    fn family(&self) -> &'static str;

    fn scheme(&self) -> SignatureScheme;

    /// Chain-native address of the MPC-derived public key.
    fn derived_address(
        &self,
        public_key: &near_crypto::PublicKey,
    ) -> color_eyre::eyre::Result<String>;

    /// Fetches chain context (nonce/blockhash/fees/...) for the derived
    /// sender and builds the unsigned transaction.
    fn build(
        &self,
        chain: &ResolvedChain,
        derived_public_key: &near_crypto::PublicKey,
        owner: &str,
        derivation_path: &str,
        latency: ExecutionLatency,
    ) -> color_eyre::eyre::Result<BuiltTransaction>;

    /// Combines the unsigned transaction with the MPC signatures and
    /// broadcasts it; returns the destination-chain transaction id.
    fn assemble_and_broadcast(
        &self,
        chain: &ResolvedChain,
        unsigned_tx: &serde_json::Value,
        signatures: &[crate::mpc::MpcSignatureResponse],
    ) -> color_eyre::eyre::Result<String> {
        assemble_and_broadcast(chain, unsigned_tx, signatures)
    }
}

/// Assembly + broadcast dispatched by the chain's family - the entry point
/// for `transaction broadcast`, where no action spec exists (the unsigned
/// transaction comes from an envelope).
pub fn assemble_and_broadcast(
    chain: &ResolvedChain,
    unsigned_tx: &serde_json::Value,
    signatures: &[crate::mpc::MpcSignatureResponse],
) -> color_eyre::eyre::Result<String> {
    match chain.family.as_str() {
        evm::FAMILY => evm::assemble_and_broadcast(chain, unsigned_tx, signatures),
        svm::FAMILY => svm::assemble_and_broadcast(chain, unsigned_tx, signatures),
        aptos::FAMILY => aptos::assemble_and_broadcast(chain, unsigned_tx, signatures),
        sui::FAMILY => sui::assemble_and_broadcast(chain, unsigned_tx, signatures),
        utxo::FAMILY => utxo::assemble_and_broadcast(chain, unsigned_tx, signatures),
        ton::FAMILY => ton::assemble_and_broadcast(chain, unsigned_tx, signatures),
        other => Err(color_eyre::eyre::eyre!(
            "Chain family '{other}' is not supported for assembly/broadcast"
        )),
    }
}

