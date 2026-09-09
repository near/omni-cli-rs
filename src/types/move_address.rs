/// A 32-byte Move-style address (Aptos, Sui): `0x`-prefixed hex up to 64
/// nibbles, left-padded (so `0x1` is the framework address).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveAddress(pub [u8; 32]);

impl MoveAddress {
    pub fn to_hex(self) -> String {
        format!("0x{}", hex::encode(self.0))
    }
}

impl std::str::FromStr for MoveAddress {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex_str = s.trim().strip_prefix("0x").unwrap_or(s.trim());
        if hex_str.is_empty() || hex_str.len() > 64 {
            return Err(format!(
                "Invalid Move address '{s}': expected up to 64 hex characters"
            ));
        }
        let padded = format!("{hex_str:0>64}");
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(&padded, &mut bytes)
            .map_err(|err| format!("Invalid Move address '{s}': {err}"))?;
        Ok(Self(bytes))
    }
}

impl std::fmt::Display for MoveAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl interactive_clap::ToCli for MoveAddress {
    type CliVariant = MoveAddress;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn pads_short_addresses() {
        let one = MoveAddress::from_str("0x1").unwrap();
        assert_eq!(one.0[31], 1);
        assert_eq!(one.0[..31], [0u8; 31]);
        assert_eq!(
            one.to_hex(),
            "0x0000000000000000000000000000000000000000000000000000000000000001"
        );
        assert!(MoveAddress::from_str("0x").is_err());
        assert!(MoveAddress::from_str("0xzz").is_err());
        assert!(MoveAddress::from_str(&format!("0x{}", "1".repeat(65))).is_err());
    }
}
