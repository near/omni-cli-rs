//! Shared HTTP plumbing for the per-family RPC clients: the base URL is
//! parsed and validated once at client construction, one blocking reqwest
//! client is reused across calls, and the JSON-RPC 2.0 envelope is typed
//! instead of hand-built JSON.

use color_eyre::eyre::{WrapErr, eyre};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// For JSON-RPC methods that take no parameters (serializes to `[]`).
pub const NO_PARAMS: [(); 0] = [];

/// Deserializes a u64 that APIs encode either as a JSON number or as a
/// decimal string (Aptos, Sui, and toncenter all do the latter in places).
pub fn u64_from_number_or_string<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(u64),
        String(String),
    }
    match Raw::deserialize(deserializer)? {
        Raw::Number(value) => Ok(value),
        Raw::String(value) => value.parse().map_err(serde::de::Error::custom),
    }
}

/// A u64 result value that may arrive as a number or a decimal string.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct FlexU64(#[serde(deserialize_with = "u64_from_number_or_string")] pub u64);

/// A JSON-RPC 2.0 endpoint (EVM, SVM, Sui).
pub struct JsonRpcClient {
    http: reqwest::blocking::Client,
    url: reqwest::Url,
    label: &'static str,
}

#[derive(Serialize)]
struct JsonRpcRequest<'a, P: Serialize> {
    jsonrpc: &'static str,
    id: u32,
    method: &'a str,
    params: P,
}

#[derive(Deserialize)]
struct JsonRpcResponse<R> {
    error: Option<JsonRpcError>,
    result: Option<R>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    data: Option<serde_json::Value>,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)?;
        if let Some(data) = &self.data {
            write!(f, ": {data}")?;
        }
        Ok(())
    }
}

impl JsonRpcClient {
    pub fn new(url: &str, label: &'static str) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            http: reqwest::blocking::Client::new(),
            url: url
                .parse()
                .wrap_err_with(|| format!("Invalid {label} URL: '{url}'"))?,
            label,
        })
    }

    pub fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> color_eyre::eyre::Result<R> {
        let response: JsonRpcResponse<R> = self
            .http
            .post(self.url.clone())
            .json(&JsonRpcRequest {
                jsonrpc: "2.0",
                id: 1,
                method,
                params,
            })
            .send()
            .wrap_err_with(|| format!("Failed to reach the {} at {}", self.label, self.url))?
            .json()
            .wrap_err_with(|| {
                format!(
                    "Unexpected {} response for {method} from {}",
                    self.label, self.url
                )
            })?;
        if let Some(error) = response.error {
            return Err(eyre!("{} error from {method}: {error}", self.label));
        }
        response
            .result
            .ok_or_else(|| eyre!("The {} response for {method} has no result", self.label))
    }
}

/// A REST-style HTTP API (Aptos fullnode, toncenter, Esplora).
pub struct RestClient {
    http: reqwest::blocking::Client,
    base_url: String,
    label: &'static str,
}

impl RestClient {
    pub fn new(base_url: &str, label: &'static str) -> color_eyre::eyre::Result<Self> {
        let _: reqwest::Url = base_url
            .parse()
            .wrap_err_with(|| format!("Invalid {label} URL: '{base_url}'"))?;
        Ok(Self {
            http: reqwest::blocking::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            label,
        })
    }

    pub fn get(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.http.get(format!("{}{path}", self.base_url))
    }

    pub fn post(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.http.post(format!("{}{path}", self.base_url))
    }

    /// Sends the request; returns the status and the raw body, with
    /// reach/read failures labeled by API.
    pub fn send(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> color_eyre::eyre::Result<(reqwest::StatusCode, String)> {
        let response = request
            .send()
            .wrap_err_with(|| format!("Failed to reach the {} at {}", self.label, self.base_url))?;
        let status = response.status();
        let body = response
            .text()
            .wrap_err_with(|| format!("Failed to read the {} response", self.label))?;
        Ok((status, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flexible_u64_accepts_numbers_and_strings() {
        assert_eq!(serde_json::from_str::<FlexU64>("42").unwrap().0, 42);
        assert_eq!(serde_json::from_str::<FlexU64>("\"42\"").unwrap().0, 42);
        assert!(serde_json::from_str::<FlexU64>("\"nope\"").is_err());
    }

    #[test]
    fn json_rpc_request_wire_format() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "eth_chainId",
            params: NO_PARAMS,
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}"#
        );
    }

    #[test]
    fn json_rpc_error_renders_message_code_and_data() {
        let response: JsonRpcResponse<u64> = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"nonce too low","data":"0x1"}}"#,
        )
        .unwrap();
        assert_eq!(
            response.error.unwrap().to_string(),
            "nonce too low (code -32000): \"0x1\""
        );
    }
}
