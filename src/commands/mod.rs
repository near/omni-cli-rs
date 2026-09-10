pub mod account;
pub mod config;
pub mod proposal;
pub mod self_update;
pub mod transaction;

/// An output table in the same style near-cli-rs uses for its listings.
pub(crate) fn new_table() -> prettytable::Table {
    let mut table = prettytable::Table::new();
    table.set_format(*prettytable::format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
    table
}

/// A title row in omni's accent color (the same violet as the interactive
/// prompts); prettytable's `Fg=>` style letters only reach the 16 basic
/// colors, so the 256-color index is set on each cell directly.
pub(crate) fn title_row(titles: &[&str]) -> prettytable::Row {
    prettytable::Row::new(
        titles
            .iter()
            .map(|title| {
                prettytable::Cell::new(title)
                    .with_style(prettytable::Attr::ForegroundColor(u32::from(
                        crate::ACCENT_ANSI,
                    )))
                    .with_style(prettytable::Attr::Bold)
            })
            .collect(),
    )
}

/// The shared derivation-path prompt, pre-filled with the configurable
/// default (`default_derivation_path` in the omni config).
pub(crate) fn input_derivation_path() -> color_eyre::eyre::Result<Option<String>> {
    let default_path = crate::config::load_or_init()?.default_derivation_path;
    let path = inquire::Text::new("Derivation path (determines the acting foreign account):")
        .with_initial_value(&default_path)
        .prompt()?;
    Ok(Some(path))
}
