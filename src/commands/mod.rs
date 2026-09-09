pub mod account;
pub mod proposal;
pub mod transaction;

/// The shared derivation-path prompt, pre-filled with the configurable
/// default (`default_derivation_path` in the omni config).
pub(crate) fn input_derivation_path() -> color_eyre::eyre::Result<Option<String>> {
    let default_path = crate::config::load_or_init()?.default_derivation_path;
    let path = inquire::Text::new("Derivation path (determines the acting foreign account):")
        .with_initial_value(&default_path)
        .prompt()?;
    Ok(Some(path))
}
