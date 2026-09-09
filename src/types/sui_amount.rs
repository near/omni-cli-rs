const MIST_PER_SUI: u64 = 1_000_000_000;

/// A Sui amount entered as `0.5 SUI` or `1000 mist`.
/// The unit is required to avoid mist/SUI footguns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SuiAmount {
    pub mist: u64,
}

impl std::str::FromStr for SuiAmount {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mist = super::parse_move_style_amount(s, "SUI", &["sui"], &["mist"], MIST_PER_SUI)?;
        Ok(Self { mist })
    }
}

impl std::fmt::Display for SuiAmount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            super::format_move_style_amount(self.mist, "SUI", "mist", MIST_PER_SUI)
        )
    }
}

impl interactive_clap::ToCli for SuiAmount {
    type CliVariant = SuiAmount;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parses_and_displays() {
        assert_eq!(SuiAmount::from_str("0.5 SUI").unwrap().mist, 500_000_000);
        assert_eq!(SuiAmount::from_str("1000 mist").unwrap().mist, 1000);
        assert!(SuiAmount::from_str("100").is_err());
        assert_eq!(SuiAmount { mist: 500_000_000 }.to_string(), "0.5 SUI");
    }
}
