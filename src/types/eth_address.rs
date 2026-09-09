#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EthAddress(pub alloy_primitives::Address);

impl EthAddress {
    pub fn as_bytes(&self) -> [u8; 20] {
        self.0.into_array()
    }
}

impl From<[u8; 20]> for EthAddress {
    fn from(bytes: [u8; 20]) -> Self {
        Self(alloy_primitives::Address::from(bytes))
    }
}

impl std::str::FromStr for EthAddress {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let address: alloy_primitives::Address = s
            .parse()
            .map_err(|err| format!("Invalid EVM address '{s}': {err}"))?;
        Ok(Self(address))
    }
}

impl std::fmt::Display for EthAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.to_checksum(None))
    }
}

impl interactive_clap::ToCli for EthAddress {
    type CliVariant = EthAddress;
}
