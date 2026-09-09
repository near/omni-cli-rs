//! Minimal blocking JSON-RPC client for Sui fullnodes.

use color_eyre::eyre::{WrapErr, eyre};

fn call(
    rpc_url: &str,
    method: &str,
    params: serde_json::Value,
) -> color_eyre::eyre::Result<serde_json::Value> {
    let response: serde_json::Value = reqwest::blocking::Client::new()
        .post(rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        }))
        .send()
        .wrap_err_with(|| format!("Failed to reach Sui RPC at {rpc_url}"))?
        .json()
        .wrap_err_with(|| format!("Invalid JSON from Sui RPC at {rpc_url}"))?;

    if let Some(error) = response.get("error").filter(|e| !e.is_null()) {
        return Err(eyre!("Sui RPC error from {method}: {error}"));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| eyre!("Sui RPC response for {method} has no result"))
}

fn parse_u64(value: &serde_json::Value) -> color_eyre::eyre::Result<u64> {
    match value {
        serde_json::Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| eyre!("Expected a u64, got: {value}")),
        serde_json::Value::String(s) => s
            .parse()
            .wrap_err_with(|| format!("Expected a u64 string, got: {s}")),
        other => Err(eyre!("Expected a u64, got: {other}")),
    }
}

pub fn reference_gas_price(rpc_url: &str) -> color_eyre::eyre::Result<u64> {
    parse_u64(&call(
        rpc_url,
        "suix_getReferenceGasPrice",
        serde_json::json!([]),
    )?)
}

/// A SUI gas coin owned by an address.
#[derive(Debug, Clone)]
pub struct SuiCoin {
    pub object_id: String,
    pub version: u64,
    pub digest_base58: String,
    pub balance: u64,
}

/// SUI coins owned by `owner` (first page, up to 50 - plenty for gas
/// selection).
pub fn sui_coins(rpc_url: &str, owner: &str) -> color_eyre::eyre::Result<Vec<SuiCoin>> {
    let result = call(
        rpc_url,
        "suix_getCoins",
        serde_json::json!([owner, "0x2::sui::SUI", null, 50]),
    )?;
    let coins = result["data"]
        .as_array()
        .ok_or_else(|| eyre!("suix_getCoins returned no data array"))?;
    coins
        .iter()
        .map(|coin| {
            Ok(SuiCoin {
                object_id: coin["coinObjectId"]
                    .as_str()
                    .ok_or_else(|| eyre!("coin has no coinObjectId"))?
                    .to_string(),
                version: parse_u64(&coin["version"])?,
                digest_base58: coin["digest"]
                    .as_str()
                    .ok_or_else(|| eyre!("coin has no digest"))?
                    .to_string(),
                balance: parse_u64(&coin["balance"])?,
            })
        })
        .collect()
}

/// Broadcasts a signed transaction; returns the transaction digest.
pub fn execute_transaction(
    rpc_url: &str,
    tx_bytes_base64: &str,
    signature_base64: &str,
) -> color_eyre::eyre::Result<String> {
    let result = call(
        rpc_url,
        "sui_executeTransactionBlock",
        serde_json::json!([tx_bytes_base64, [signature_base64]]),
    )?;
    result["digest"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| eyre!("sui_executeTransactionBlock returned no digest: {result}"))
}
