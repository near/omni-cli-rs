//! Bitcoin address encoding/decoding: bech32(m) segwit and base58check
//! legacy addresses, to and from scriptPubKeys.

use bech32::Hrp;
use color_eyre::eyre::{WrapErr, eyre};
use ripemd::Ripemd160;
use sha2::{Digest, Sha256};

/// Network parameters relevant to addressing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtcNetwork {
    Mainnet,
    Testnet,
}

impl BtcNetwork {
    pub fn from_near_network(near_network: &str) -> Self {
        if near_network == "mainnet" {
            Self::Mainnet
        } else {
            Self::Testnet
        }
    }

    fn hrp(self) -> Hrp {
        match self {
            Self::Mainnet => Hrp::parse_unchecked("bc"),
            Self::Testnet => Hrp::parse_unchecked("tb"),
        }
    }

    fn p2pkh_version(self) -> u8 {
        match self {
            Self::Mainnet => 0x00,
            Self::Testnet => 0x6f,
        }
    }

    fn p2sh_version(self) -> u8 {
        match self {
            Self::Mainnet => 0x05,
            Self::Testnet => 0xc4,
        }
    }
}

pub fn hash160(data: &[u8]) -> [u8; 20] {
    Ripemd160::digest(Sha256::digest(data)).into()
}

pub fn sha256d(data: &[u8]) -> [u8; 32] {
    Sha256::digest(Sha256::digest(data)).into()
}

/// Compressed SEC1 form of an MPC-derived key (64-byte uncompressed point).
pub fn compress_public_key(uncompressed: &[u8; 64]) -> [u8; 33] {
    let mut compressed = [0u8; 33];
    compressed[0] = 0x02 + (uncompressed[63] & 1);
    compressed[1..].copy_from_slice(&uncompressed[..32]);
    compressed
}

/// The derived address: native segwit P2WPKH over the compressed key.
pub fn p2wpkh_address(compressed_public_key: &[u8; 33], network: BtcNetwork) -> String {
    bech32::segwit::encode_v0(network.hrp(), &hash160(compressed_public_key))
        .expect("a 20-byte program always encodes")
}

/// The P2WPKH scriptPubKey (`OP_0 <20-byte pubkey hash>`), used for change.
pub fn p2wpkh_script_pubkey(compressed_public_key: &[u8; 33]) -> Vec<u8> {
    let pkh = hash160(compressed_public_key);
    let mut script = vec![0x00, 0x14];
    script.extend_from_slice(&pkh);
    script
}

/// The BIP143 script code of a P2WPKH input: the classic P2PKH script
/// (`OP_DUP OP_HASH160 <pkh> OP_EQUALVERIFY OP_CHECKSIG`).
pub fn p2wpkh_script_code(compressed_public_key: &[u8; 33]) -> Vec<u8> {
    let pkh = hash160(compressed_public_key);
    let mut script = vec![0x76, 0xa9, 0x14];
    script.extend_from_slice(&pkh);
    script.extend_from_slice(&[0x88, 0xac]);
    script
}

/// Parses a Bitcoin address (bech32/bech32m segwit or base58check legacy)
/// into its scriptPubKey, validating the network.
pub fn address_to_script_pubkey(
    address: &str,
    network: BtcNetwork,
) -> color_eyre::eyre::Result<Vec<u8>> {
    let address = address.trim();

    if let Ok((hrp, witness_version, program)) = bech32::segwit::decode(address) {
        if hrp != network.hrp() {
            return Err(eyre!(
                "Address '{address}' is for a different Bitcoin network (expected hrp '{}')",
                network.hrp()
            ));
        }
        let mut script = Vec::with_capacity(2 + program.len());
        script.push(match witness_version.to_u8() {
            0 => 0x00,
            version @ 1..=16 => 0x50 + version,
            other => return Err(eyre!("Unsupported witness version {other}")),
        });
        script.push(program.len() as u8);
        script.extend_from_slice(&program);
        return Ok(script);
    }

    // base58check: version byte + payload(20) + sha256d checksum(4)
    let decoded = bs58::decode(address)
        .into_vec()
        .wrap_err_with(|| format!("'{address}' is neither a bech32 nor a base58 address"))?;
    if decoded.len() != 25 {
        return Err(eyre!("Invalid base58 address length for '{address}'"));
    }
    let (body, checksum) = decoded.split_at(21);
    if sha256d(body)[..4] != *checksum {
        return Err(eyre!("Invalid base58check checksum for '{address}'"));
    }
    let version = body[0];
    let payload = &body[1..];
    if version == network.p2pkh_version() {
        let mut script = vec![0x76, 0xa9, 0x14];
        script.extend_from_slice(payload);
        script.extend_from_slice(&[0x88, 0xac]);
        Ok(script)
    } else if version == network.p2sh_version() {
        let mut script = vec![0xa9, 0x14];
        script.extend_from_slice(payload);
        script.push(0x87);
        Ok(script)
    } else {
        Err(eyre!(
            "Address version byte 0x{version:02x} does not match the selected network"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_p2wpkh_vector() {
        // BIP173 example: the genesis-block style pubkey
        let pubkey =
            hex::decode("0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
                .unwrap();
        let pubkey: [u8; 33] = pubkey.try_into().unwrap();
        assert_eq!(
            p2wpkh_address(&pubkey, BtcNetwork::Mainnet),
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4"
        );
        // Round trip through the parser
        let script =
            address_to_script_pubkey("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4", BtcNetwork::Mainnet)
                .unwrap();
        assert_eq!(script, p2wpkh_script_pubkey(&pubkey));
        assert_eq!(script[..2], [0x00, 0x14]);
    }

    #[test]
    fn parses_legacy_and_rejects_wrong_network() {
        // The genesis coinbase address (P2PKH mainnet)
        let script = address_to_script_pubkey(
            "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa",
            BtcNetwork::Mainnet,
        )
        .unwrap();
        assert_eq!(script[0], 0x76);
        assert_eq!(script.len(), 25);

        assert!(
            address_to_script_pubkey("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa", BtcNetwork::Testnet)
                .is_err()
        );
        assert!(
            address_to_script_pubkey(
                "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
                BtcNetwork::Testnet
            )
            .is_err()
        );
    }
}
