const OCTAS_PER_APT: u64 = 100_000_000;

/// An Aptos amount entered as `0.5 APT` or `1000 octas`.
/// The unit is required to avoid octas/APT footguns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AptAmount {
    pub octas: u64,
}

impl std::str::FromStr for AptAmount {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let octas = super::parse_move_style_amount(s, "APT", &["apt"], &["octas", "octa"], OCTAS_PER_APT)?;
        Ok(Self { octas })
    }
}

impl std::fmt::Display for AptAmount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", super::format_move_style_amount(self.octas, "APT", "octas", OCTAS_PER_APT))
    }
}

impl interactive_clap::ToCli for AptAmount {
    type CliVariant = AptAmount;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parses_and_displays() {
        assert_eq!(AptAmount::from_str("0.5 APT").unwrap().octas, 50_000_000);
        assert_eq!(AptAmount::from_str("1000 octas").unwrap().octas, 1000);
        assert!(AptAmount::from_str("100").is_err());
        assert!(AptAmount::from_str("0.5 octas").is_err());
        assert_eq!(AptAmount { octas: 50_000_000 }.to_string(), "0.5 APT");
        assert_eq!(AptAmount { octas: 12 }.to_string(), "12 octas");
    }
}
