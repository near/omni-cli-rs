//! Minimal blocking JSON-RPC client for EVM chains. Intentionally thin
//! (plain reqwest) - omni-cli keeps heavy chain SDKs out of the dependency
//! tree, matching omni-transaction-rs's own philosophy.

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
        .wrap_err_with(|| format!("Failed to reach EVM RPC at {rpc_url}"))?
        .json()
        .wrap_err_with(|| format!("Invalid JSON from EVM RPC at {rpc_url}"))?;

    if let Some(error) = response.get("error").filter(|e| !e.is_null()) {
        return Err(eyre!("EVM RPC error from {method}: {error}"));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| eyre!("EVM RPC response for {method} has no result"))
}

fn parse_quantity(value: &serde_json::Value) -> color_eyre::eyre::Result<u128> {
    let s = value
        .as_str()
        .ok_or_else(|| eyre!("Expected a hex quantity string, got: {value}"))?;
    let hex_str = s.strip_prefix("0x").unwrap_or(s);
    u128::from_str_radix(hex_str, 16).wrap_err_with(|| format!("Invalid hex quantity: {s}"))
}

fn address_hex(address: [u8; 20]) -> String {
    format!("0x{}", hex::encode(address))
}

pub fn chain_id(rpc_url: &str) -> color_eyre::eyre::Result<u64> {
    Ok(parse_quantity(&call(rpc_url, "eth_chainId", serde_json::json!([]))?)? as u64)
}

pub fn nonce(rpc_url: &str, address: [u8; 20]) -> color_eyre::eyre::Result<u64> {
    Ok(parse_quantity(&call(
        rpc_url,
        "eth_getTransactionCount",
        serde_json::json!([address_hex(address), "pending"]),
    )?)? as u64)
}

pub fn balance(rpc_url: &str, address: [u8; 20]) -> color_eyre::eyre::Result<u128> {
    parse_quantity(&call(
        rpc_url,
        "eth_getBalance",
        serde_json::json!([address_hex(address), "latest"]),
    )?)
}

pub fn gas_price(rpc_url: &str) -> color_eyre::eyre::Result<u128> {
    parse_quantity(&call(rpc_url, "eth_gasPrice", serde_json::json!([]))?)
}

pub fn max_priority_fee(rpc_url: &str) -> u128 {
    const DEFAULT_TIP: u128 = 1_500_000_000; // 1.5 gwei
    call(rpc_url, "eth_maxPriorityFeePerGas", serde_json::json!([]))
        .and_then(|v| parse_quantity(&v))
        .unwrap_or(DEFAULT_TIP)
}

pub fn estimate_gas(
    rpc_url: &str,
    from: [u8; 20],
    to: [u8; 20],
    value_wei: u128,
    data: &[u8],
) -> color_eyre::eyre::Result<u128> {
    let mut tx = serde_json::json!({
        "from": address_hex(from),
        "to": address_hex(to),
        "value": format!("0x{value_wei:x}"),
    });
    if !data.is_empty() {
        tx["data"] = serde_json::Value::String(format!("0x{}", hex::encode(data)));
    }
    parse_quantity(&call(rpc_url, "eth_estimateGas", serde_json::json!([tx]))?).wrap_err(
        "Gas estimation failed - the transaction would likely revert as constructed \
         (check the target address, calldata, and the derived account's balance)",
    )
}

pub fn send_raw_transaction(rpc_url: &str, raw_tx: &[u8]) -> color_eyre::eyre::Result<String> {
    let result = call(
        rpc_url,
        "eth_sendRawTransaction",
        serde_json::json!([format!("0x{}", hex::encode(raw_tx))]),
    )?;
    result
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| eyre!("eth_sendRawTransaction returned a non-string result: {result}"))
}
