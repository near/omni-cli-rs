/// A base58 Solana address CLI argument, validated on parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolanaAddressArg(pub omni_transaction::solana::types::SolanaAddress);

impl std::str::FromStr for SolanaAddressArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        omni_transaction::solana::types::SolanaAddress::from_base58(s.trim())
            .map(Self)
            .map_err(|err| format!("Invalid Solana address '{s}': {err}"))
    }
}

impl std::fmt::Display for SolanaAddressArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.to_base58())
    }
}

impl interactive_clap::ToCli for SolanaAddressArg {
    type CliVariant = SolanaAddressArg;
}
