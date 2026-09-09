//! EVM chain family: payload building, MPC signature assembly, and broadcast
//! on top of omni-transaction-rs EIP-1559 builders.

pub mod abi;
pub mod rpc;

use color_eyre::eyre::{ContextCompat, WrapErr, eyre};
use omni_transaction::evm::EVMTransaction;

use crate::chains::{BuiltTransaction, ChainAdapter, ExecutionLatency, SignatureScheme};
use crate::config::ResolvedChain;
use crate::mpc::MpcSignatureResponse;

pub const FAMILY: &str = "evm";

/// What the user asked for, before chain context (nonce/fees) is known.
#[derive(Debug, Clone)]
pub struct EvmActionSpec {
    pub to: [u8; 20],
    pub value_wei: u128,
    pub data: Vec<u8>,
    /// Human-readable summary of the action for renders,
    /// e.g. `transfer 0.5 ETH to 0x...` or the decoded function call.
    pub summary: String,
}

pub struct EvmAdapter {
    pub spec: EvmActionSpec,
}

impl ChainAdapter for EvmAdapter {
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
        let pk = crate::mpc::secp256k1_bytes(public_key)?;
        Ok(checksum(address_from_derived_pk(&pk)))
    }

    fn build(
        &self,
        chain: &ResolvedChain,
        derived_public_key: &near_crypto::PublicKey,
        owner: &str,
        derivation_path: &str,
        latency: ExecutionLatency,
    ) -> color_eyre::eyre::Result<BuiltTransaction> {
        let pk = crate::mpc::secp256k1_bytes(derived_public_key)?;
        let derived_address = address_from_derived_pk(&pk);
        let params = fetch_tx_params(chain, derived_address, &self.spec, latency).wrap_err_with(
            || {
                format!(
                    "Failed to prepare the transaction on '{}' for derived sender {}",
                    chain.chain_key,
                    checksum(derived_address)
                )
            },
        )?;
        let chain_id = chain.chain_id.wrap_err_with(|| {
            format!(
                "EVM chain '{}' ({}) needs a chain_id in the omni config",
                chain.chain_key, chain.near_network
            )
        })?;
        let tx = build_unsigned(chain_id, &self.spec, &params);
        let payload = sighash(&tx);
        let display = describe(chain, &tx, owner, derivation_path, derived_address, &self.spec);
        Ok(BuiltTransaction {
            unsigned_tx: serde_json::to_value(&tx)?,
            payloads: vec![payload.to_vec()],
            display,
        })
    }

}

/// Combines the unsigned EVM transaction with the MPC signature and
/// broadcasts it; returns the destination-chain transaction hash.
pub fn assemble_and_broadcast(
    chain: &ResolvedChain,
    unsigned_tx: &serde_json::Value,
    signatures: &[MpcSignatureResponse],
) -> color_eyre::eyre::Result<String> {
    let tx: EVMTransaction = serde_json::from_value(unsigned_tx.clone())
        .wrap_err("Failed to deserialize the unsigned EVM transaction")?;
    let response = signatures
        .first()
        .wrap_err("No MPC signature available to assemble")?;
    let signature = signature_from_mpc(response)?;
    let raw_tx = tx.build_with_signature(&signature);
    rpc::send_raw_transaction(&chain.rpc_url, &raw_tx)
}

#[derive(Debug, Clone, Copy)]
pub struct EvmTxParams {
    pub nonce: u64,
    pub gas_limit: u128,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
}

/// Fetches nonce and fee parameters for the derived sender. The fee ceiling
/// headroom depends on execution latency: a DAO-gated payload may broadcast
/// days later, so it gets a much higher `max_fee_per_gas` ceiling (EIP-1559
/// refunds the unused part).
pub fn fetch_tx_params(
    chain: &ResolvedChain,
    from: [u8; 20],
    spec: &EvmActionSpec,
    latency: ExecutionLatency,
) -> color_eyre::eyre::Result<EvmTxParams> {
    let expected_chain_id = chain.chain_id.wrap_err_with(|| {
        format!(
            "EVM chain '{}' ({}) needs a chain_id in the omni config",
            chain.chain_key, chain.near_network
        )
    })?;
    let actual_chain_id = rpc::chain_id(&chain.rpc_url)
        .wrap_err_with(|| format!("Failed to query chain id from {}", chain.rpc_url))?;
    if actual_chain_id != expected_chain_id {
        return Err(eyre!(
            "The RPC at {} reports chain id {actual_chain_id}, but the registry entry \
             says {expected_chain_id}. Fix the chain registry before signing anything.",
            chain.rpc_url,
        ));
    }

    let nonce = rpc::nonce(&chain.rpc_url, from)?;
    let base_estimate = rpc::gas_price(&chain.rpc_url)?;
    let max_priority_fee_per_gas = rpc::max_priority_fee(&chain.rpc_url);
    let headroom_multiplier = match latency {
        ExecutionLatency::Immediate => 2,
        ExecutionLatency::Governance => 5,
    };
    let max_fee_per_gas = base_estimate
        .saturating_mul(headroom_multiplier)
        .saturating_add(max_priority_fee_per_gas);

    let gas_limit = if spec.data.is_empty() {
        // Plain native transfer to an EOA; if the target turns out to be a
        // contract, estimate_gas covers it below via the fallback.
        match rpc::estimate_gas(&chain.rpc_url, from, spec.to, spec.value_wei, &[]) {
            Ok(estimate) => estimate.saturating_mul(13) / 10,
            Err(_) => 21_000,
        }
    } else {
        let estimate =
            rpc::estimate_gas(&chain.rpc_url, from, spec.to, spec.value_wei, &spec.data)?;
        estimate.saturating_mul(13) / 10
    };

    Ok(EvmTxParams {
        nonce,
        gas_limit,
        max_fee_per_gas,
        max_priority_fee_per_gas,
    })
}

pub fn build_unsigned(chain_id: u64, spec: &EvmActionSpec, params: &EvmTxParams) -> EVMTransaction {
    EVMTransaction {
        chain_id,
        nonce: params.nonce,
        to: Some(spec.to),
        value: spec.value_wei,
        input: spec.data.clone(),
        gas_limit: params.gas_limit,
        max_fee_per_gas: params.max_fee_per_gas,
        max_priority_fee_per_gas: params.max_priority_fee_per_gas,
        access_list: vec![],
    }
}

/// The 32-byte payload the MPC signs: keccak256 of the EIP-1559 signing RLP.
pub fn sighash(tx: &EVMTransaction) -> [u8; 32] {
    alloy_primitives::keccak256(tx.build_for_signing()).0
}

/// EVM address of an MPC-derived secp256k1 key (64-byte uncompressed point).
pub fn address_from_derived_pk(public_key: &[u8; 64]) -> [u8; 20] {
    let hash = alloy_primitives::keccak256(public_key);
    let mut address = [0u8; 20];
    address.copy_from_slice(&hash[12..]);
    address
}

pub fn checksum(address: [u8; 20]) -> String {
    alloy_primitives::Address::from(address).to_checksum(None)
}

fn format_native(wei: u128, chain: &ResolvedChain) -> String {
    let unit = 10u128.pow(chain.decimals as u32);
    if wei == 0 {
        format!("0 {}", chain.symbol)
    } else if wei.is_multiple_of(unit) {
        format!("{} {}", wei / unit, chain.symbol)
    } else {
        format!(
            "{}.{} {}",
            wei / unit,
            format!("{:0>width$}", wei % unit, width = chain.decimals as usize)
                .trim_end_matches('0'),
            chain.symbol
        )
    }
}

fn format_gwei(wei: u128) -> String {
    format!(
        "{}.{:02} gwei",
        wei / 1_000_000_000,
        (wei % 1_000_000_000) / 10_000_000
    )
}

/// Human-readable render of the unsigned transaction - shown before signing
/// (sign-as-account) and in proposal reviews (sign-as-dao).
pub fn describe(
    chain: &ResolvedChain,
    tx: &EVMTransaction,
    owner: &str,
    derivation_path: &str,
    derived_address: [u8; 20],
    spec: &EvmActionSpec,
) -> String {
    let to = tx.to.map(checksum).unwrap_or_else(|| "<create>".to_string());
    let max_cost_wei = tx.gas_limit.saturating_mul(tx.max_fee_per_gas);
    format!(
        "\n\
         Unsigned {chain_key} transaction (chain id {chain_id}, NEAR {near_network}):\n\
         ------------------------------------------------------------\n\
         action:        {summary}\n\
         from:          {from} (derived: {owner} / \"{derivation_path}\")\n\
         to:            {to}\n\
         value:         {value}\n\
         calldata:      {calldata}\n\
         nonce:         {nonce}\n\
         gas limit:     {gas_limit}\n\
         max fee:       {max_fee} (priority tip {tip})\n\
         max gas cost:  {max_cost} (unused fees are refunded)\n\
         signing hash:  0x{payload}\n\
         ------------------------------------------------------------",
        chain_key = chain.chain_key,
        chain_id = tx.chain_id,
        near_network = chain.near_network,
        summary = spec.summary,
        from = checksum(derived_address),
        value = format_native(tx.value, chain),
        calldata = if tx.input.is_empty() {
            "(none)".to_string()
        } else {
            format!("0x{} ({} bytes)", hex::encode(&tx.input), tx.input.len())
        },
        nonce = tx.nonce,
        gas_limit = tx.gas_limit,
        max_fee = format_gwei(tx.max_fee_per_gas),
        tip = format_gwei(tx.max_priority_fee_per_gas),
        max_cost = format_native(max_cost_wei, chain),
        payload = hex::encode(sighash(tx)),
    )
}

/// Converts the MPC secp256k1 response into an EIP-1559 signature: r is the
/// x-coordinate of big_r (compressed point without the parity byte), s is the
/// scalar, v is the recovery id (y-parity).
pub fn signature_from_mpc(
    response: &MpcSignatureResponse,
) -> color_eyre::eyre::Result<omni_transaction::evm::types::Signature> {
    let MpcSignatureResponse::Secp256k1 {
        big_r,
        s,
        recovery_id,
    } = response
    else {
        return Err(eyre!(
            "Expected a secp256k1 MPC signature for an EVM chain, got an ed25519 one"
        ));
    };
    let big_r = hex::decode(&big_r.affine_point).wrap_err("MPC signature big_r is not valid hex")?;
    if big_r.len() != 33 {
        return Err(eyre!(
            "MPC signature big_r must be a 33-byte compressed point, got {} bytes",
            big_r.len()
        ));
    }
    let s = hex::decode(&s.scalar).wrap_err("MPC signature s is not valid hex")?;
    if s.len() != 32 {
        return Err(eyre!(
            "MPC signature s must be a 32-byte scalar, got {} bytes",
            s.len()
        ));
    }
    Ok(omni_transaction::evm::types::Signature {
        v: *recovery_id as u64,
        r: big_r[1..].to_vec(),
        s,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::{SigningKey, signature::hazmat::PrehashSigner};

    fn test_chain(rpc_url: &str) -> ResolvedChain {
        ResolvedChain {
            chain_key: "sepolia-test".to_string(),
            family: FAMILY.to_string(),
            near_network: "testnet".to_string(),
            rpc_url: rpc_url.to_string(),
            chain_id: Some(11155111),
            explorer_tx_url: None,
            symbol: "ETH".to_string(),
            decimals: 18,
        }
    }

    /// Round-trip through a real secp256k1 key: sign the sighash the way the
    /// MPC does (big_r as a compressed point, s scalar, recovery id), run our
    /// conversion + assembly, and confirm the signature recovers to the
    /// address our derivation computes for that key.
    #[test]
    fn mpc_signature_assembles_and_recovers_to_derived_address() {
        let signing_key = SigningKey::from_bytes(&[0x42u8; 32].into()).unwrap();
        let verifying_key = signing_key.verifying_key();

        // The address our code derives from the MPC public key
        let uncompressed = verifying_key.to_encoded_point(false);
        let mut pk64 = [0u8; 64];
        pk64.copy_from_slice(&uncompressed.as_bytes()[1..]);
        let expected_address = address_from_derived_pk(&pk64);

        let spec = EvmActionSpec {
            to: [0xde; 20],
            value_wei: 1_000_000_000_000_000,
            data: vec![0xa9, 0x05, 0x9c, 0xbb],
            summary: "test".to_string(),
        };
        let params = EvmTxParams {
            nonce: 7,
            gas_limit: 50_000,
            max_fee_per_gas: 20_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
        };
        let tx = build_unsigned(11155111, &spec, &params);
        let payload = sighash(&tx);

        // Sign the payload like the MPC would report it
        let (signature, recovery_id): (k256::ecdsa::Signature, k256::ecdsa::RecoveryId) =
            signing_key.sign_prehash(&payload).unwrap();
        let r_bytes: [u8; 32] = signature.r().to_bytes().into();
        let s_bytes: [u8; 32] = signature.s().to_bytes().into();
        let response = MpcSignatureResponse::Secp256k1 {
            big_r: crate::mpc::AffinePoint {
                affine_point: format!(
                    "{:02x}{}",
                    2 + (recovery_id.to_byte() & 1),
                    hex::encode(r_bytes)
                ),
            },
            s: crate::mpc::Scalar {
                scalar: hex::encode(s_bytes),
            },
            recovery_id: recovery_id.to_byte(),
        };

        let omni_signature = signature_from_mpc(&response).unwrap();
        assert_eq!(omni_signature.v, recovery_id.to_byte() as u64);
        assert_eq!(omni_signature.r, r_bytes.to_vec());
        assert_eq!(omni_signature.s, s_bytes.to_vec());

        // The assembled raw transaction is signed-RLP; independently confirm
        // the (payload, signature) pair recovers to the expected address.
        let recovered =
            k256::ecdsa::VerifyingKey::recover_from_prehash(&payload, &signature, recovery_id)
                .unwrap();
        let recovered_point = recovered.to_encoded_point(false);
        let mut recovered_pk64 = [0u8; 64];
        recovered_pk64.copy_from_slice(&recovered_point.as_bytes()[1..]);
        assert_eq!(address_from_derived_pk(&recovered_pk64), expected_address);

        let raw_tx = tx.build_with_signature(&omni_signature);
        assert_eq!(raw_tx[0], 0x02); // EIP-1559 type byte
        assert!(raw_tx.len() > tx.build_for_signing().len());
    }

    /// Serves canned JSON-RPC responses on localhost and checks nonce, fee
    /// headroom (higher ceiling for Governance latency), gas estimation
    /// margin, and the chain-id sanity check.
    #[test]
    fn fetch_tx_params_from_mock_rpc() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let rpc_url = format!("http://{}", listener.local_addr().unwrap());

        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            // 2 latency runs x 5 calls + the chain-id mismatch run
            for _ in 0..11 {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buffer = [0u8; 4096];
                let n = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..n]).to_string();
                let result = if request.contains("eth_chainId") {
                    "\"0xaa36a7\"" // 11155111
                } else if request.contains("eth_getTransactionCount") {
                    "\"0x11\"" // 17
                } else if request.contains("eth_gasPrice") {
                    "\"0x3b9aca00\"" // 1 gwei
                } else if request.contains("eth_maxPriorityFeePerGas") {
                    "\"0x5f5e100\"" // 0.1 gwei
                } else if request.contains("eth_estimateGas") {
                    "\"0x7530\"" // 30000
                } else {
                    "null"
                };
                let body = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{result}}}");
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let chain = test_chain(&rpc_url);
        let spec = EvmActionSpec {
            to: [0xde; 20],
            value_wei: 0,
            data: vec![0x84, 0x56, 0xcb, 0x59],
            summary: "test".to_string(),
        };

        let immediate =
            fetch_tx_params(&chain, [0xaa; 20], &spec, ExecutionLatency::Immediate).unwrap();
        assert_eq!(immediate.nonce, 17);
        assert_eq!(immediate.max_priority_fee_per_gas, 100_000_000);
        // 2 * 1 gwei + 0.1 gwei
        assert_eq!(immediate.max_fee_per_gas, 2_100_000_000);
        // 30000 * 1.3
        assert_eq!(immediate.gas_limit, 39_000);

        let governance =
            fetch_tx_params(&chain, [0xaa; 20], &spec, ExecutionLatency::Governance).unwrap();
        // 5 * 1 gwei + 0.1 gwei - much more headroom for days of voting
        assert_eq!(governance.max_fee_per_gas, 5_100_000_000);

        // Registry/chain-id mismatch must be a hard error
        let wrong_chain = ResolvedChain {
            chain_id: Some(1),
            ..test_chain(&rpc_url)
        };
        let error = fetch_tx_params(&wrong_chain, [0xaa; 20], &spec, ExecutionLatency::Immediate)
            .unwrap_err();
        assert!(error.to_string().contains("chain id"));

        drop(server);
    }
}
