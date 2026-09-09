/// A TON address CLI argument (friendly base64 form), validated on parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TonAddressArg(pub omni_transaction::ton::types::TonAddress);

impl std::str::FromStr for TonAddressArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.trim()
            .parse()
            .map(Self)
            .map_err(|err| format!("Invalid TON address '{s}': {err}"))
    }
}

impl std::fmt::Display for TonAddressArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl interactive_clap::ToCli for TonAddressArg {
    type CliVariant = TonAddressArg;
}
