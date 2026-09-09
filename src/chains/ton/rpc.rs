//! Blocking client for the toncenter HTTP API (v2), typed: the `ok/result/
//! error` envelope and the payloads are serde structs.

use base64::Engine;
use color_eyre::eyre::{WrapErr, eyre};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::chains::http::{RestClient, u64_from_number_or_string};

pub struct Client {
    rest: RestClient,
}

/// Every toncenter v2 response is wrapped in this envelope.
#[derive(Deserialize)]
struct TonCenterResponse<T> {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    result: Option<T>,
}

/// `getWalletInformation` payload. Most fields are absent for wallets that
/// are not deployed yet.
#[derive(Deserialize)]
struct WalletInformation {
    #[serde(default)]
    account_state: Option<String>,
    #[serde(default)]
    seqno: Option<u32>,
    #[serde(default, deserialize_with = "optional_nanotons")]
    balance: Option<u64>,
}

fn optional_nanotons<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    u64_from_number_or_string(deserializer).map(Some)
}

#[derive(Serialize)]
struct SendBocRequest {
    boc: String,
}

#[derive(Deserialize)]
struct SendBocResult {
    hash: String,
}

/// Wallet state as toncenter reports it.
#[derive(Debug, Clone)]
pub struct WalletInfo {
    pub deployed: bool,
    pub seqno: u32,
    pub balance_nanotons: u64,
}

impl Client {
    pub fn new(base_url: &str) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            rest: RestClient::new(base_url, "toncenter API")?,
        })
    }

    /// Parses the toncenter envelope; toncenter signals errors with
    /// `ok: false` (sometimes still with a 2xx status), so the envelope is
    /// authoritative and the HTTP status is only a fallback.
    fn unwrap_envelope<T: DeserializeOwned>(
        status: reqwest::StatusCode,
        body: &str,
        what: &str,
    ) -> color_eyre::eyre::Result<T> {
        let response: TonCenterResponse<T> = serde_json::from_str(body)
            .wrap_err_with(|| format!("Unexpected toncenter response ({status}) from {what}"))?;
        if !response.ok {
            return Err(eyre!(
                "toncenter error from {what}: {}",
                response.error.as_deref().unwrap_or("unknown error")
            ));
        }
        response
            .result
            .ok_or_else(|| eyre!("toncenter response from {what} has no result"))
    }

    pub fn wallet_information(&self, address: &str) -> color_eyre::eyre::Result<WalletInfo> {
        let request = self
            .rest
            .get("/getWalletInformation")
            .query(&[("address", address)]);
        let (status, body) = self.rest.send(request)?;
        let info: WalletInformation = Self::unwrap_envelope(status, &body, "getWalletInformation")?;
        Ok(WalletInfo {
            deployed: info.account_state.as_deref() == Some("active"),
            seqno: info.seqno.unwrap_or(0),
            balance_nanotons: info.balance.unwrap_or(0),
        })
    }

    /// Broadcasts a Bag-of-Cells external message; returns the message hash
    /// (hex).
    pub fn send_boc(&self, boc: &[u8]) -> color_eyre::eyre::Result<String> {
        let engine = base64::engine::general_purpose::STANDARD;
        let request = self.rest.post("/sendBocReturnHash").json(&SendBocRequest {
            boc: engine.encode(boc),
        });
        let (status, body) = self.rest.send(request)?;
        let result: SendBocResult = Self::unwrap_envelope(status, &body, "sendBocReturnHash")?;
        let hash_bytes = engine
            .decode(&result.hash)
            .wrap_err("toncenter returned an unparseable message hash")?;
        Ok(hex::encode(hash_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_deployed_and_uninitialized_wallets() {
        let deployed: TonCenterResponse<WalletInformation> = serde_json::from_str(
            r#"{"ok":true,"result":{"wallet":true,"balance":"123456789",
                "account_state":"active","wallet_type":"wallet v5 r1","seqno":7}}"#,
        )
        .unwrap();
        let info = deployed.result.unwrap();
        assert_eq!(info.account_state.as_deref(), Some("active"));
        assert_eq!(info.seqno, Some(7));
        assert_eq!(info.balance, Some(123_456_789));

        // Not-yet-deployed wallets have almost no fields.
        let empty: TonCenterResponse<WalletInformation> = serde_json::from_str(
            r#"{"ok":true,"result":{"wallet":false,"balance":"0","account_state":"uninitialized"}}"#,
        )
        .unwrap();
        let info = empty.result.unwrap();
        assert_eq!(info.seqno, None);
        assert_eq!(info.balance, Some(0));

        let error: TonCenterResponse<WalletInformation> =
            serde_json::from_str(r#"{"ok":false,"error":"Incorrect address","code":416}"#).unwrap();
        assert!(!error.ok);
        assert_eq!(error.error.as_deref(), Some("Incorrect address"));
    }
}
