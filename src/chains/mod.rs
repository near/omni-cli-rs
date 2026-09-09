pub mod aptos;
pub mod evm;
pub mod sui;
pub mod svm;
pub mod ton;
pub mod utxo;

use crate::config::ResolvedChain;

/// Replaces a JSON byte array (numbers 0-255) with a `0x...` hex string -
/// envelope prettification for fields the upstream serde emits as arrays.
pub(crate) fn bytes_array_to_hex(value: &mut serde_json::Value) {
    let Some(items) = value.as_array() else {
        return;
    };
    let bytes: Option<Vec<u8>> = items
        .iter()
        .map(|item| item.as_u64().and_then(|n| u8::try_from(n).ok()))
        .collect();
    if let Some(bytes) = bytes {
        *value = serde_json::Value::String(format!("0x{}", hex::encode(bytes)));
    }
}

/// Reverse of [`bytes_array_to_hex`]: turns a `0x...` string back into a byte
/// array so the upstream serde can deserialize it. Non-strings are untouched
/// (already in array form, e.g. an envelope from an older CLI).
pub(crate) fn hex_to_bytes_array(value: &mut serde_json::Value) -> color_eyre::eyre::Result<()> {
    if let Some(s) = value.as_str() {
        let bytes = hex::decode(s.strip_prefix("0x").unwrap_or(s))
            .map_err(|err| color_eyre::eyre::eyre!("Invalid hex in the envelope: {err}"))?;
        *value = serde_json::Value::from(bytes);
    }
    Ok(())
}

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

/// The MPC key domain a family's payloads are signed with.
pub fn family_scheme(family: &str) -> color_eyre::eyre::Result<SignatureScheme> {
    match family {
        evm::FAMILY | utxo::FAMILY => Ok(SignatureScheme::Secp256k1),
        svm::FAMILY | aptos::FAMILY | sui::FAMILY | ton::FAMILY => Ok(SignatureScheme::Ed25519),
        other => Err(color_eyre::eyre::eyre!("Unknown chain family '{other}'")),
    }
}

/// Recomputes the MPC signing payloads from an envelope's unsigned tx,
/// dispatched by family - the byte-equality half of `proposal review`.
pub fn signing_payloads_from_envelope(
    family: &str,
    unsigned_tx: &serde_json::Value,
) -> color_eyre::eyre::Result<Vec<Vec<u8>>> {
    match family {
        evm::FAMILY => evm::signing_payloads_from_envelope(unsigned_tx),
        svm::FAMILY => svm::signing_payloads_from_envelope(unsigned_tx),
        aptos::FAMILY => aptos::signing_payloads_from_envelope(unsigned_tx),
        sui::FAMILY => sui::signing_payloads_from_envelope(unsigned_tx),
        utxo::FAMILY => utxo::signing_payloads_from_envelope(unsigned_tx),
        ton::FAMILY => ton::signing_payloads_from_envelope(unsigned_tx),
        other => Err(color_eyre::eyre::eyre!("Unknown chain family '{other}'")),
    }
}

/// The chain-native derived address, from the MPC-derived keys of both
/// domains (families pick the one they need).
pub fn derived_address_for_chain(
    chain: &ResolvedChain,
    secp256k1_pk: &[u8; 64],
    ed25519_pk: &[u8; 32],
) -> color_eyre::eyre::Result<String> {
    Ok(match chain.family.as_str() {
        evm::FAMILY => evm::checksum(evm::address_from_derived_pk(secp256k1_pk)),
        svm::FAMILY => omni_transaction::solana::types::SolanaAddress(*ed25519_pk).to_base58(),
        utxo::FAMILY => utxo::address::p2wpkh_address(
            &utxo::address::compress_public_key(secp256k1_pk),
            utxo::address::BtcNetwork::from_near_network(&chain.near_network),
        ),
        aptos::FAMILY => format!(
            "0x{}",
            hex::encode(aptos::address_from_derived_pk(ed25519_pk))
        ),
        sui::FAMILY => omni_transaction::sui::utils::derive_sui_address(
            omni_transaction::sui::types::SignatureScheme::Ed25519,
            ed25519_pk,
        )
        .to_hex(),
        ton::FAMILY => ton::wallet_address_string(ed25519_pk, chain),
        other => {
            return Err(color_eyre::eyre::eyre!("Unknown chain family '{other}'"));
        }
    })
}

/// The derived address's native balance on the chain, formatted.
pub fn derived_balance_for_chain(
    chain: &ResolvedChain,
    secp256k1_pk: &[u8; 64],
    ed25519_pk: &[u8; 32],
) -> color_eyre::eyre::Result<String> {
    let address = derived_address_for_chain(chain, secp256k1_pk, ed25519_pk)?;
    let base_units: u128 = match chain.family.as_str() {
        evm::FAMILY => {
            evm::rpc::balance(&chain.rpc_url, evm::address_from_derived_pk(secp256k1_pk))?
        }
        svm::FAMILY => u128::from(svm::rpc::balance(&chain.rpc_url, &address)?),
        utxo::FAMILY => u128::from(
            utxo::rpc::utxos(&chain.rpc_url, &address)?
                .iter()
                .map(|utxo| utxo.value_sats)
                .sum::<u64>(),
        ),
        aptos::FAMILY => u128::from(aptos::rpc::apt_balance(&chain.rpc_url, &address)),
        sui::FAMILY => u128::from(
            sui::rpc::sui_coins(&chain.rpc_url, &address)?
                .iter()
                .map(|coin| coin.balance)
                .sum::<u64>(),
        ),
        ton::FAMILY => {
            u128::from(ton::rpc::wallet_information(&chain.rpc_url, &address)?.balance_nanotons)
        }
        other => {
            return Err(color_eyre::eyre::eyre!("Unknown chain family '{other}'"));
        }
    };
    Ok(format_units(base_units, chain.decimals, &chain.symbol))
}

/// `1234500000000000000 / 18 decimals` -> `1.2345 ETH`.
pub fn format_units(base_units: u128, decimals: u8, symbol: &str) -> String {
    let unit = 10u128.pow(u32::from(decimals));
    if base_units.is_multiple_of(unit) {
        format!("{} {symbol}", base_units / unit)
    } else {
        format!(
            "{}.{} {symbol}",
            base_units / unit,
            format!("{:0>width$}", base_units % unit, width = decimals as usize)
                .trim_end_matches('0'),
        )
    }
}
