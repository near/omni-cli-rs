/// Hex-encoded bytes (`0x`-prefixed or bare), e.g. raw EVM calldata.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HexBytes(pub Vec<u8>);

impl std::str::FromStr for HexBytes {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let hex_str = s.strip_prefix("0x").unwrap_or(s);
        if hex_str.is_empty() {
            return Ok(Self(Vec::new()));
        }
        hex::decode(hex_str)
            .map(Self)
            .map_err(|err| format!("Invalid hex string: {err}"))
    }
}

impl std::fmt::Display for HexBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{}", hex::encode(&self.0))
    }
}

impl interactive_clap::ToCli for HexBytes {
    type CliVariant = HexBytes;
}
