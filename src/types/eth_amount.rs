const WEI_PER_ETH: u128 = 1_000_000_000_000_000_000;
const WEI_PER_GWEI: u128 = 1_000_000_000;

/// A native-token amount entered as `0.5 ETH`, `2 gwei`, or `1000 wei`.
/// The unit is required to avoid wei/ETH footguns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EthAmount {
    pub wei: u128,
}

impl EthAmount {
    pub fn from_wei(wei: u128) -> Self {
        Self { wei }
    }
}

impl std::str::FromStr for EthAmount {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (number_part, unit) = match s.find(|c: char| c.is_ascii_alphabetic()) {
            Some(idx) => (s[..idx].trim(), s[idx..].trim().to_lowercase()),
            None => {
                return Err(format!(
                    "A unit is required (e.g. '0.5 ETH', '2 gwei', '1000 wei'), got: '{s}'"
                ));
            }
        };
        let multiplier = match unit.as_str() {
            "eth" | "ether" => WEI_PER_ETH,
            "gwei" => WEI_PER_GWEI,
            "wei" => 1,
            _ => {
                return Err(format!(
                    "Unknown unit '{unit}' (expected ETH, gwei, or wei)"
                ));
            }
        };
        let wei = parse_decimal_scaled(number_part, multiplier)
            .ok_or_else(|| format!("Invalid amount: '{s}'"))?;
        Ok(Self { wei })
    }
}

/// Parses a decimal string like "0.5" scaled by `multiplier`, without floats.
fn parse_decimal_scaled(number: &str, multiplier: u128) -> Option<u128> {
    let (int_part, frac_part) = match number.split_once('.') {
        Some((i, f)) => (i, f),
        None => (number, ""),
    };
    let int_part = if int_part.is_empty() { "0" } else { int_part };
    if !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let int_value: u128 = int_part.parse().ok()?;
    let mut result = int_value.checked_mul(multiplier)?;
    if !frac_part.is_empty() {
        // frac_part scaled: multiplier must be divisible by 10^len(frac_part)
        let scale = 10u128.checked_pow(frac_part.len() as u32)?;
        if !multiplier.is_multiple_of(scale) {
            return None; // more precision than the unit allows
        }
        let frac_value: u128 = frac_part.parse().ok()?;
        result = result.checked_add(frac_value.checked_mul(multiplier / scale)?)?;
    }
    Some(result)
}

impl std::fmt::Display for EthAmount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.wei == 0 {
            write!(f, "0 ETH")
        } else if self.wei.is_multiple_of(WEI_PER_ETH) {
            write!(f, "{} ETH", self.wei / WEI_PER_ETH)
        } else if self.wei >= WEI_PER_ETH / 1_000_000 {
            write!(
                f,
                "{}.{} ETH",
                self.wei / WEI_PER_ETH,
                format!("{:0>18}", self.wei % WEI_PER_ETH).trim_end_matches('0')
            )
        } else {
            write!(f, "{} wei", self.wei)
        }
    }
}

impl interactive_clap::ToCli for EthAmount {
    type CliVariant = EthAmount;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parses_eth() {
        assert_eq!(
            EthAmount::from_str("0.5 ETH").unwrap().wei,
            500_000_000_000_000_000
        );
        assert_eq!(EthAmount::from_str("1 eth").unwrap().wei, WEI_PER_ETH);
        assert_eq!(EthAmount::from_str("2 gwei").unwrap().wei, 2 * WEI_PER_GWEI);
        assert_eq!(EthAmount::from_str("1000 wei").unwrap().wei, 1000);
        assert_eq!(EthAmount::from_str("0 ETH").unwrap().wei, 0);
    }

    #[test]
    fn rejects_unitless_and_overprecise() {
        assert!(EthAmount::from_str("100").is_err());
        assert!(EthAmount::from_str("0.5 wei").is_err());
    }

    #[test]
    fn display_roundtrip() {
        assert_eq!(EthAmount::from_wei(WEI_PER_ETH / 2).to_string(), "0.5 ETH");
        assert_eq!(EthAmount::from_wei(0).to_string(), "0 ETH");
        assert_eq!(EthAmount::from_wei(1000).to_string(), "1000 wei");
        assert_eq!(
            EthAmount::from_str("0.5 ETH").unwrap(),
            EthAmount::from_str(&EthAmount::from_wei(WEI_PER_ETH / 2).to_string()).unwrap()
        );
    }
}
