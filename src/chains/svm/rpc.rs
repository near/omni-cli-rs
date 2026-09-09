//! Minimal blocking JSON-RPC client for SVM chains (Solana, Fogo, ...).

use color_eyre::eyre::{WrapErr, eyre};

fn call(
    rpc_url: &str,
    method: &str,
    params: serde_json::Value,
) -> color_eyre::eyre::Result<serde_json::Value> {
    let client = reqwest::blocking::Client::new();
    let response: serde_json::Value = client
        .post(rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        }))
        .send()
        .wrap_err_with(|| format!("Failed to reach SVM RPC at {rpc_url}"))?
        .json()
        .wrap_err_with(|| format!("Invalid JSON from SVM RPC at {rpc_url}"))?;

    if let Some(error) = response.get("error").filter(|e| !e.is_null()) {
        return Err(eyre!("SVM RPC error from {method}: {error}"));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| eyre!("SVM RPC response for {method} has no result"))
}

/// Returns the latest blockhash (base58) and its last valid block height.
pub fn latest_blockhash(rpc_url: &str) -> color_eyre::eyre::Result<(String, u64)> {
    let result = call(
        rpc_url,
        "getLatestBlockhash",
        serde_json::json!([{ "commitment": "confirmed" }]),
    )?;
    let blockhash = result["value"]["blockhash"]
        .as_str()
        .ok_or_else(|| eyre!("getLatestBlockhash returned no blockhash"))?
        .to_string();
    let last_valid_block_height = result["value"]["lastValidBlockHeight"].as_u64().unwrap_or(0);
    Ok((blockhash, last_valid_block_height))
}

pub fn balance(rpc_url: &str, address_base58: &str) -> color_eyre::eyre::Result<u64> {
    let result = call(
        rpc_url,
        "getBalance",
        serde_json::json!([address_base58, { "commitment": "confirmed" }]),
    )?;
    result["value"]
        .as_u64()
        .ok_or_else(|| eyre!("getBalance returned a non-numeric value"))
}

/// Broadcasts base64-encoded wire bytes; returns the transaction signature.
pub fn send_transaction(rpc_url: &str, tx_base64: &str) -> color_eyre::eyre::Result<String> {
    let result = call(
        rpc_url,
        "sendTransaction",
        serde_json::json!([tx_base64, { "encoding": "base64" }]),
    )?;
    result
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| eyre!("sendTransaction returned a non-string result: {result}"))
}
