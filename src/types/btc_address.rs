/// A Bitcoin address CLI argument. Full validation (including the network
/// check) happens at build time; this only rejects obvious garbage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtcAddressArg(pub String);

impl std::str::FromStr for BtcAddressArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.len() < 14 || s.len() > 90 || !s.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!("'{s}' does not look like a Bitcoin address"));
        }
        Ok(Self(s.to_string()))
    }
}

impl std::fmt::Display for BtcAddressArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl interactive_clap::ToCli for BtcAddressArg {
    type CliVariant = BtcAddressArg;
}
