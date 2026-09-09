pub mod apt_amount;
pub mod btc_address;
pub mod btc_amount;
pub mod eth_address;
pub mod eth_amount;
pub mod hex_bytes;
pub mod move_address;
pub mod sol_amount;
pub mod solana_address;
pub mod sui_amount;
pub mod ton_address;
pub mod ton_amount;

/// Parses `"0.5 <UNIT>"` / `"1000 <base-unit>"` into base units without
/// floats. Shared by the u64-based amount types (APT/octas, SUI/mist).
pub(crate) fn parse_move_style_amount(
    input: &str,
    unit_display: &str,
    unit_aliases: &[&str],
    base_unit_aliases: &[&str],
    base_per_unit: u64,
) -> Result<u64, String> {
    let input = input.trim();
    let (number_part, unit) = match input.find(|c: char| c.is_ascii_alphabetic()) {
        Some(idx) => (input[..idx].trim(), input[idx..].trim().to_lowercase()),
        None => {
            return Err(format!(
                "A unit is required (e.g. '0.5 {unit_display}', '1000 {}'), got: '{input}'",
                base_unit_aliases[0]
            ));
        }
    };
    let multiplier = if unit_aliases.contains(&unit.as_str()) {
        base_per_unit
    } else if base_unit_aliases.contains(&unit.as_str()) {
        1
    } else {
        return Err(format!(
            "Unknown unit '{unit}' (expected {unit_display} or {})",
            base_unit_aliases[0]
        ));
    };

    let (int_part, frac_part) = match number_part.split_once('.') {
        Some((i, f)) => (i, f),
        None => (number_part, ""),
    };
    let int_part = if int_part.is_empty() { "0" } else { int_part };
    if !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(format!("Invalid amount: '{input}'"));
    }
    let mut result = int_part
        .parse::<u64>()
        .ok()
        .and_then(|value| value.checked_mul(multiplier))
        .ok_or_else(|| format!("Amount out of range: '{input}'"))?;
    if !frac_part.is_empty() {
        let scale = 10u64
            .checked_pow(frac_part.len() as u32)
            .filter(|scale| multiplier % scale == 0)
            .ok_or_else(|| format!("Too much precision for the unit: '{input}'"))?;
        let frac_value: u64 = frac_part
            .parse()
            .map_err(|_| format!("Invalid amount: '{input}'"))?;
        result = result
            .checked_add(frac_value * (multiplier / scale))
            .ok_or_else(|| format!("Amount out of range: '{input}'"))?;
    }
    Ok(result)
}

pub(crate) fn format_move_style_amount(
    base_units: u64,
    unit_display: &str,
    base_unit_display: &str,
    base_per_unit: u64,
) -> String {
    let decimals = base_per_unit.ilog10() as usize;
    if base_units == 0 {
        format!("0 {unit_display}")
    } else if base_units.is_multiple_of(base_per_unit) {
        format!("{} {unit_display}", base_units / base_per_unit)
    } else if base_units >= base_per_unit / 1_000 {
        format!(
            "{}.{} {unit_display}",
            base_units / base_per_unit,
            format!("{:0>decimals$}", base_units % base_per_unit).trim_end_matches('0')
        )
    } else {
        format!("{base_units} {base_unit_display}")
    }
}
