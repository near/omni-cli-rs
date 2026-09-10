//! Informational output inside the interactive flow.
//!
//! near-cli-rs renders `tracing` events in the prompt gutter (`├  ...`, with
//! `│  ` on continuation lines) so that progress notes, warnings, and
//! previews read as part of the dialogue instead of breaking its left edge.
//! Anything printed while prompts are still coming goes through here;
//! results printed after the flow (tables, the echoed command, broadcast
//! outcomes) stay plain. `--quiet` silences these, as it does in near-cli-rs.

/// A progress or context line: `├  text`.
pub fn info(text: impl AsRef<str>) {
    tracing::info!(target: "omni", "{}", gutter(text.as_ref()));
}

/// A warning line: `├  Warning: text` (yellow).
pub fn warn(text: impl AsRef<str>) {
    tracing::warn!(target: "omni", "{}", gutter(text.as_ref()));
}

/// Multi-line messages keep the gutter on every line.
fn gutter(text: &str) -> String {
    text.trim_matches('\n')
        .lines()
        .collect::<Vec<_>>()
        .join("\n│  ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn gutter_prefixes_continuation_lines_only() {
        assert_eq!(super::gutter("\none\ntwo\n"), "one\n│  two");
        assert_eq!(super::gutter("single"), "single");
    }
}
