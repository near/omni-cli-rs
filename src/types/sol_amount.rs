const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// A native SVM amount entered as `0.5 SOL` or `5000 lamports`.
/// The unit is required to avoid lamports/SOL footguns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SolAmount {
    pub lamports: u64,
}

impl std::str::FromStr for SolAmount {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (number_part, unit) = match s.find(|c: char| c.is_ascii_alphabetic()) {
            Some(idx) => (s[..idx].trim(), s[idx..].trim().to_lowercase()),
            None => {
                return Err(format!(
                    "A unit is required (e.g. '0.5 SOL', '5000 lamports'), got: '{s}'"
                ));
            }
        };
        let multiplier = match unit.as_str() {
            "sol" => LAMPORTS_PER_SOL,
            "lamports" | "lamport" => 1,
            _ => {
                return Err(format!(
                    "Unknown unit '{unit}' (expected SOL or lamports)"
                ));
            }
        };
        let (int_part, frac_part) = match number_part.split_once('.') {
            Some((i, f)) => (i, f),
            None => (number_part, ""),
        };
        let int_part = if int_part.is_empty() { "0" } else { int_part };
        if !int_part.bytes().all(|b| b.is_ascii_digit())
            || !frac_part.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(format!("Invalid amount: '{s}'"));
        }
        let mut lamports = int_part
            .parse::<u64>()
            .ok()
            .and_then(|v| v.checked_mul(multiplier))
            .ok_or_else(|| format!("Amount out of range: '{s}'"))?;
        if !frac_part.is_empty() {
            let scale = 10u64
                .checked_pow(frac_part.len() as u32)
                .filter(|scale| multiplier % scale == 0)
                .ok_or_else(|| format!("Too much precision for the unit: '{s}'"))?;
            let frac_value: u64 = frac_part.parse().map_err(|_| format!("Invalid amount: '{s}'"))?;
            lamports = lamports
                .checked_add(frac_value * (multiplier / scale))
                .ok_or_else(|| format!("Amount out of range: '{s}'"))?;
        }
        Ok(Self { lamports })
    }
}

impl std::fmt::Display for SolAmount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.lamports == 0 {
            write!(f, "0 SOL")
        } else if self.lamports.is_multiple_of(LAMPORTS_PER_SOL) {
            write!(f, "{} SOL", self.lamports / LAMPORTS_PER_SOL)
        } else if self.lamports >= LAMPORTS_PER_SOL / 1_000 {
            write!(
                f,
                "{}.{} SOL",
                self.lamports / LAMPORTS_PER_SOL,
                format!("{:0>9}", self.lamports % LAMPORTS_PER_SOL).trim_end_matches('0')
            )
        } else {
            write!(f, "{} lamports", self.lamports)
        }
    }
}

impl interactive_clap::ToCli for SolAmount {
    type CliVariant = SolAmount;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parses_and_displays() {
        assert_eq!(SolAmount::from_str("0.5 SOL").unwrap().lamports, 500_000_000);
        assert_eq!(SolAmount::from_str("5000 lamports").unwrap().lamports, 5000);
        assert_eq!(SolAmount::from_str("1 sol").unwrap().lamports, LAMPORTS_PER_SOL);
        assert!(SolAmount::from_str("100").is_err());
        assert!(SolAmount::from_str("0.5 lamports").is_err());
        assert_eq!(
            SolAmount { lamports: 500_000_000 }.to_string(),
            "0.5 SOL"
        );
        assert_eq!(
            SolAmount::from_str(&SolAmount { lamports: 123_456_789 }.to_string()).unwrap(),
            SolAmount { lamports: 123_456_789 }
        );
    }
}
