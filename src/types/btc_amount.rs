const SATS_PER_BTC: u64 = 100_000_000;

/// A Bitcoin amount entered as `0.5 BTC` or `1000 sats`.
/// The unit is required to avoid sats/BTC footguns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BtcAmount {
    pub sats: u64,
}

impl std::str::FromStr for BtcAmount {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let sats =
            super::parse_move_style_amount(s, "BTC", &["btc"], &["sats", "sat"], SATS_PER_BTC)?;
        Ok(Self { sats })
    }
}

impl std::fmt::Display for BtcAmount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            super::format_move_style_amount(self.sats, "BTC", "sats", SATS_PER_BTC)
        )
    }
}

impl interactive_clap::ToCli for BtcAmount {
    type CliVariant = BtcAmount;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parses_and_displays() {
        assert_eq!(BtcAmount::from_str("0.5 BTC").unwrap().sats, 50_000_000);
        assert_eq!(BtcAmount::from_str("1000 sats").unwrap().sats, 1000);
        assert!(BtcAmount::from_str("100").is_err());
        assert_eq!(BtcAmount { sats: 50_000_000 }.to_string(), "0.5 BTC");
    }
}
