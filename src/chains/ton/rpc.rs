//! Minimal blocking client for the toncenter HTTP API (v2).

use base64::Engine;
use color_eyre::eyre::{WrapErr, eyre};

fn check_ok(body: serde_json::Value, what: &str) -> color_eyre::eyre::Result<serde_json::Value> {
    if body["ok"].as_bool() != Some(true) {
        return Err(eyre!(
            "toncenter error from {what}: {}",
            body["error"].as_str().unwrap_or("unknown error")
        ));
    }
    Ok(body["result"].clone())
}

/// Wallet state as toncenter reports it.
#[derive(Debug, Clone)]
pub struct WalletInfo {
    pub deployed: bool,
    pub seqno: u32,
    pub balance_nanotons: u64,
}

pub fn wallet_information(base_url: &str, address: &str) -> color_eyre::eyre::Result<WalletInfo> {
    let url = format!(
        "{}/getWalletInformation?address={address}",
        base_url.trim_end_matches('/')
    );
    let body: serde_json::Value = reqwest::blocking::Client::new()
        .get(&url)
        .send()
        .wrap_err_with(|| format!("Failed to reach toncenter at {url}"))?
        .json()
        .wrap_err("Invalid JSON from toncenter")?;
    let result = check_ok(body, "getWalletInformation")?;
    let deployed = result["account_state"].as_str() == Some("active");
    let seqno = result["seqno"].as_u64().unwrap_or(0) as u32;
    let balance_nanotons = result["balance"]
        .as_str()
        .and_then(|balance| balance.parse().ok())
        .or_else(|| result["balance"].as_u64())
        .unwrap_or(0);
    Ok(WalletInfo {
        deployed,
        seqno,
        balance_nanotons,
    })
}

/// Broadcasts a Bag-of-Cells external message; returns the message hash (hex).
pub fn send_boc(base_url: &str, boc: &[u8]) -> color_eyre::eyre::Result<String> {
    let url = format!("{}/sendBocReturnHash", base_url.trim_end_matches('/'));
    let engine = base64::engine::general_purpose::STANDARD;
    let body: serde_json::Value = reqwest::blocking::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "boc": engine.encode(boc) }))
        .send()
        .wrap_err_with(|| format!("Failed to reach toncenter at {url}"))?
        .json()
        .wrap_err("Invalid JSON from toncenter")?;
    let result = check_ok(body, "sendBocReturnHash")?;
    let hash_base64 = result["hash"]
        .as_str()
        .ok_or_else(|| eyre!("sendBocReturnHash returned no hash"))?;
    let hash_bytes = engine
        .decode(hash_base64)
        .wrap_err("toncenter returned an unparseable message hash")?;
    Ok(hex::encode(hash_bytes))
}
