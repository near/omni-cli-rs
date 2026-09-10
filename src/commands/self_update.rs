//! `omni self-update`: replaces the running binary with the latest GitHub
//! release (mirrors `near extensions self-update`, but top-level).

use color_eyre::eyre::WrapErr;

const REPO_OWNER: &str = "near";
const REPO_NAME: &str = "omni-cli-rs";

#[cfg(windows)]
const BIN_NAME: &str = "omni.exe";
#[cfg(not(windows))]
const BIN_NAME: &str = "omni";

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(input_context = near_cli_rs::GlobalContext)]
#[interactive_clap(output_context = SelfUpdateContext)]
pub struct SelfUpdate;

#[derive(Debug, Clone)]
pub struct SelfUpdateContext;

impl SelfUpdateContext {
    pub fn from_previous_context(
        _previous_context: near_cli_rs::GlobalContext,
        _scope: &<SelfUpdate as interactive_clap::ToInteractiveClapContextScope>::InteractiveClapContextScope,
    ) -> color_eyre::eyre::Result<Self> {
        let status = self_update::backends::github::Update::configure()
            .repo_owner(REPO_OWNER)
            .repo_name(REPO_NAME)
            // Release archives follow the cargo-dist layout:
            // <name>-<target>/<bin> inside <name>-<target>.tar.gz
            .bin_path_in_archive(
                format!("{REPO_NAME}-{}/{BIN_NAME}", self_update::get_target()).as_str(),
            )
            .bin_name(BIN_NAME)
            .show_download_progress(true)
            .current_version(self_update::cargo_crate_version!())
            .build()
            .wrap_err("Failed to configure the self-updater")?
            .update()
            .wrap_err_with(|| {
                format!(
                    "Failed to self-update from https://github.com/{REPO_OWNER}/{REPO_NAME}/releases"
                )
            })?;

        match status {
            self_update::VersionStatus::Updated(release) => {
                eprintln!(
                    "\nUpdated omni to v{release}!\n\
                     What's new: https://github.com/{REPO_OWNER}/{REPO_NAME}/releases/tag/v{release}"
                );
            }
            self_update::VersionStatus::UpToDate(version) => {
                eprintln!("\nomni v{version} is already the latest release.");
            }
            other => eprintln!("\nSelf-update finished: {other:?}"),
        }
        Ok(Self)
    }
}
