//! Blocking JSON-RPC client for EVM chains. Intentionally thin (plain
//! reqwest, no heavy chain SDK - matching omni-transaction-rs's own
//! philosophy), but typed: requests and responses are serde structs.

use color_eyre::eyre::WrapErr;
use omni_transaction::evm::types::Address;
use serde::{Deserialize, Serialize};

use crate::chains::http::{JsonRpcClient, NO_PARAMS};

pub struct Client {
    rpc: JsonRpcClient,
}

/// An EVM JSON-RPC quantity: a `0x`-prefixed hex string.
#[derive(Debug, Clone, Copy)]
struct Quantity(u128);

impl<'de> Deserialize<'de> for Quantity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let hex_str = value.strip_prefix("0x").unwrap_or(&value);
        u128::from_str_radix(hex_str, 16)
            .map(Quantity)
            .map_err(|_| serde::de::Error::custom(format!("invalid hex quantity: '{value}'")))
    }
}

/// An EVM JSON-RPC byte string: `0x`-prefixed hex of arbitrary length.
#[derive(Debug, Clone)]
struct Bytes(Vec<u8>);

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let hex_str = value.strip_prefix("0x").unwrap_or(&value);
        hex::decode(hex_str)
            .map(Bytes)
            .map_err(|_| serde::de::Error::custom(format!("invalid hex bytes: '{value}'")))
    }
}

fn quantity(value: u128) -> String {
    format!("0x{value:x}")
}

fn address(address: Address) -> String {
    format!("0x{}", hex::encode(address))
}

/// `Address` is a bare `[u8; 20]` alias in omni-transaction, so a derived
/// serde impl would emit a number array; the wire wants `0x...` hex.
fn serialize_address<S: serde::Serializer>(
    address: &Address,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&self::address(*address))
}

fn bytes(data: &[u8]) -> String {
    format!("0x{}", hex::encode(data))
}

/// The transaction object of `eth_estimateGas`.
#[derive(Serialize)]
struct CallRequest {
    #[serde(serialize_with = "serialize_address")]
    from: Address,
    #[serde(serialize_with = "serialize_address")]
    to: Address,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
}

impl Client {
    pub fn new(rpc_url: &str) -> color_eyre::eyre::Result<Self> {
        Ok(Self {
            rpc: JsonRpcClient::new(rpc_url, "EVM RPC")?,
        })
    }

    pub fn chain_id(&self) -> color_eyre::eyre::Result<u64> {
        let Quantity(id) = self.rpc.call("eth_chainId", NO_PARAMS)?;
        Ok(id as u64)
    }

    pub fn nonce(&self, account: Address) -> color_eyre::eyre::Result<u64> {
        let Quantity(nonce) = self
            .rpc
            .call("eth_getTransactionCount", (address(account), "pending"))?;
        Ok(nonce as u64)
    }

    pub fn balance(&self, account: Address) -> color_eyre::eyre::Result<u128> {
        let Quantity(balance) = self
            .rpc
            .call("eth_getBalance", (address(account), "latest"))?;
        Ok(balance)
    }

    /// Deployed bytecode at `account` (empty for an EOA / nothing deployed).
    pub fn get_code(&self, account: Address) -> color_eyre::eyre::Result<Vec<u8>> {
        let Bytes(code) = self.rpc.call("eth_getCode", (address(account), "latest"))?;
        Ok(code)
    }

    /// One storage word of `account`.
    pub fn get_storage_at(
        &self,
        account: Address,
        slot: [u8; 32],
    ) -> color_eyre::eyre::Result<[u8; 32]> {
        let Bytes(word) = self.rpc.call(
            "eth_getStorageAt",
            (address(account), bytes(&slot), "latest"),
        )?;
        // Nodes return exactly 32 bytes, but left-pad defensively.
        if word.len() > 32 {
            return Err(color_eyre::eyre::eyre!(
                "eth_getStorageAt returned {} bytes, expected 32",
                word.len()
            ));
        }
        let mut out = [0u8; 32];
        out[32 - word.len()..].copy_from_slice(&word);
        Ok(out)
    }

    pub fn gas_price(&self) -> color_eyre::eyre::Result<u128> {
        let Quantity(price) = self.rpc.call("eth_gasPrice", NO_PARAMS)?;
        Ok(price)
    }

    pub fn max_priority_fee(&self) -> u128 {
        const DEFAULT_TIP: u128 = 1_500_000_000; // 1.5 gwei
        self.rpc
            .call("eth_maxPriorityFeePerGas", NO_PARAMS)
            .map_or(DEFAULT_TIP, |Quantity(tip)| tip)
    }

    pub fn estimate_gas(
        &self,
        from: Address,
        to: Address,
        value_wei: u128,
        data: &[u8],
    ) -> color_eyre::eyre::Result<u128> {
        let request = CallRequest {
            from,
            to,
            value: quantity(value_wei),
            data: (!data.is_empty()).then(|| bytes(data)),
        };
        let Quantity(gas) = self.rpc.call("eth_estimateGas", (request,)).wrap_err(
            "Gas estimation failed - the transaction would likely revert as constructed \
             (check the target address, calldata, and the derived account's balance)",
        )?;
        Ok(gas)
    }

    /// Broadcasts RLP-encoded signed transaction bytes; returns the tx hash.
    pub fn send_raw_transaction(&self, raw_tx: &[u8]) -> color_eyre::eyre::Result<String> {
        self.rpc.call("eth_sendRawTransaction", (bytes(raw_tx),))
    }
}
