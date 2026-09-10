//! Blocking JSON-RPC client for SVM chains (Solana, Fogo, ...), typed:
//! requests and responses are serde structs.

use color_eyre::eyre::WrapErr;
use serde::{Deserialize, Serialize};

use crate::chains::http::JsonRpcClient;

pub struct Client {
    rpc: JsonRpcClient,
}

/// The standard SVM RPC config object passed as the last parameter.
#[derive(Serialize)]
struct RpcConfig {
    commitment: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    encoding: Option<&'static str>,
}

const CONFIRMED: RpcConfig = RpcConfig {
    commitment: "confirmed",
    encoding: None,
};

/// SVM RPC wraps most results in `{ context, value }`.
#[derive(Deserialize)]
struct WithContext<T> {
    value: T,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestBlockhash {
    /// Base58 blockhash.
    pub blockhash: String,
    #[expect(dead_code, reason = "part of the RPC response; useful in errors")]
    pub last_valid_block_height: u64,
}

/// The `jsonParsed` layers around a durable nonce account's state.
#[derive(Deserialize)]
struct ParsedAccount {
    data: ParsedAccountData,
}

#[derive(Deserialize)]
struct ParsedAccountData {
    parsed: ParsedNonce,
}

#[derive(Deserialize)]
struct ParsedNonce {
    info: NonceAccountInfo,
}

/// An account with `encoding: base64`: `data` is `["<base64>", "base64"]`.
#[derive(Deserialize)]
struct RawAccount {
    data: (String, String),
}

/// The state of an initialized durable nonce account.
#[derive(Debug, Clone, Deserialize)]
pub struct NonceAccountInfo {
    pub authority: String,
    #[serde(rename = "blockhash")]
    pub durable_nonce_blockhash: String,
}

impl Client {
    pub fn new(rpc_url: &str) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            rpc: JsonRpcClient::new(rpc_url, "SVM RPC")?,
        })
    }

    pub fn latest_blockhash(&self) -> color_eyre::eyre::Result<LatestBlockhash> {
        let result: WithContext<LatestBlockhash> =
            self.rpc.call("getLatestBlockhash", (CONFIRMED,))?;
        Ok(result.value)
    }

    pub fn balance(&self, address_base58: &str) -> color_eyre::eyre::Result<u64> {
        let result: WithContext<u64> = self.rpc.call("getBalance", (address_base58, CONFIRMED))?;
        Ok(result.value)
    }

    /// Fetches and parses a durable nonce account. Returns `None` if the
    /// account does not exist.
    pub fn nonce_account(
        &self,
        address_base58: &str,
    ) -> color_eyre::eyre::Result<Option<NonceAccountInfo>> {
        let result: WithContext<Option<serde_json::Value>> = self.rpc.call(
            "getAccountInfo",
            (
                address_base58,
                RpcConfig {
                    commitment: "confirmed",
                    encoding: Some("jsonParsed"),
                },
            ),
        )?;
        let Some(account) = result.value else {
            return Ok(None);
        };
        let parsed: ParsedAccount = serde_json::from_value(account).wrap_err_with(|| {
            format!("Account {address_base58} exists but is not a parsed durable nonce account")
        })?;
        Ok(Some(parsed.data.parsed.info))
    }

    /// Raw account data (base64 on the wire). `None` if the account does not
    /// exist.
    pub fn account_data(&self, address_base58: &str) -> color_eyre::eyre::Result<Option<Vec<u8>>> {
        use base64::Engine;
        let result: WithContext<Option<RawAccount>> = self.rpc.call(
            "getAccountInfo",
            (
                address_base58,
                RpcConfig {
                    commitment: "confirmed",
                    encoding: Some("base64"),
                },
            ),
        )?;
        result
            .value
            .map(|account| {
                base64::engine::general_purpose::STANDARD
                    .decode(&account.data.0)
                    .wrap_err_with(|| format!("Account {address_base58}: data is not base64"))
            })
            .transpose()
    }

    /// Lamports needed to make an account of `size` bytes rent-exempt.
    pub fn minimum_rent(&self, size: u64) -> color_eyre::eyre::Result<u64> {
        self.rpc.call("getMinimumBalanceForRentExemption", (size,))
    }

    /// Broadcasts base64-encoded wire bytes; returns the transaction
    /// signature.
    pub fn send_transaction(&self, tx_base64: &str) -> color_eyre::eyre::Result<String> {
        #[derive(Serialize)]
        struct SendConfig {
            encoding: &'static str,
        }
        self.rpc.call(
            "sendTransaction",
            (tx_base64, SendConfig { encoding: "base64" }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `jsonParsed` durable nonce account, as the RPC returns it.
    #[test]
    fn parses_a_json_parsed_nonce_account() {
        let value = serde_json::json!({
            "data": {
                "parsed": {
                    "info": {
                        "authority": "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi",
                        "blockhash": "3nvbisFbFPGjNmvS9Vf8snJXWo86SLnqDEkHfHWXpWKN",
                        "feeCalculator": { "lamportsPerSignature": "5000" }
                    },
                    "type": "initialized"
                },
                "program": "nonce",
                "space": 80
            },
            "executable": false,
            "lamports": 1_500_000,
            "owner": "11111111111111111111111111111111"
        });
        let parsed: ParsedAccount = serde_json::from_value(value).unwrap();
        assert_eq!(
            parsed.data.parsed.info.authority,
            "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi"
        );
        assert_eq!(
            parsed.data.parsed.info.durable_nonce_blockhash,
            "3nvbisFbFPGjNmvS9Vf8snJXWo86SLnqDEkHfHWXpWKN"
        );

        // A regular account (base64 data) is not a parsed nonce account.
        let regular = serde_json::json!({ "data": ["aGk=", "base64"] });
        assert!(serde_json::from_value::<ParsedAccount>(regular.clone()).is_err());
        let raw: RawAccount = serde_json::from_value(regular).unwrap();
        assert_eq!(raw.data.0, "aGk=");
    }
}
