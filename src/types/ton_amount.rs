const NANOTONS_PER_TON: u64 = 1_000_000_000;

/// A TON amount entered as `0.5 TON` or `1000 nanotons`.
/// The unit is required to avoid nanotons/TON footguns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TonAmount {
    pub nanotons: u64,
}

impl std::str::FromStr for TonAmount {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let nanotons = super::parse_move_style_amount(
            s,
            "TON",
            &["ton"],
            &["nanotons", "nanoton", "nano"],
            NANOTONS_PER_TON,
        )?;
        Ok(Self { nanotons })
    }
}

impl std::fmt::Display for TonAmount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            super::format_move_style_amount(self.nanotons, "TON", "nanotons", NANOTONS_PER_TON)
        )
    }
}

impl interactive_clap::ToCli for TonAmount {
    type CliVariant = TonAmount;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parses_and_displays() {
        assert_eq!(
            TonAmount::from_str("0.5 TON").unwrap().nanotons,
            500_000_000
        );
        assert_eq!(TonAmount::from_str("1000 nanotons").unwrap().nanotons, 1000);
        assert!(TonAmount::from_str("100").is_err());
        assert_eq!(
            TonAmount {
                nanotons: 500_000_000
            }
            .to_string(),
            "0.5 TON"
        );
    }
}
