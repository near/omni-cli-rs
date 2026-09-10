#![allow(clippy::large_enum_variant)]

use color_eyre::owo_colors::OwoColorize;
use interactive_clap::ToCliArgs;
pub use near_cli_rs::CliResult;
use near_cli_rs::Verbosity;
use strum::{EnumDiscriminants, EnumIter, EnumMessage};

mod chains;
mod commands;
mod config;
mod dao;
mod envelope;
mod mpc;
mod types;

/// omni is a toolbox for controlling accounts on other chains from NEAR
/// (via a NEAR DAO or a plain NEAR account + MPC chain signatures)
#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
struct Cmd {
    /// Quiet mode
    #[interactive_clap(long)]
    quiet: bool,
    /// TEACH-ME mode
    #[interactive_clap(long)]
    teach_me: bool,
    #[interactive_clap(subcommand)]
    command: self::Command,
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
#[interactive_clap(disable_back)]
/// What are you up to? (select one of the options with the up-down arrows on your keyboard and press Enter)
pub enum Command {
    #[strum_discriminants(strum(
        message = "account     -   Inspect derived foreign accounts (addresses, balances)"
    ))]
    /// Inspect derived foreign accounts (addresses, balances)
    Account(self::commands::account::Account),
    #[strum_discriminants(strum(
        message = "transaction -   Construct, sign with MPC, and broadcast transactions on other chains"
    ))]
    /// Construct, sign with MPC, and broadcast transactions on other chains
    Transaction(self::commands::transaction::TransactionCommands),
    #[strum_discriminants(strum(
        message = "proposal    -   List, review (verify!), and vote on DAO chain-signature proposals"
    ))]
    /// List, review (verify!), and vote on DAO chain-signature proposals
    Proposal(self::commands::proposal::Proposal),
    #[strum_discriminants(strum(
        message = "config      -   Manage the omni chain registry (add chains, sync defaults, reset)"
    ))]
    /// Manage the omni chain registry (add chains, sync defaults, reset)
    Config(self::commands::config::Config),
    #[strum_discriminants(strum(
        message = "self-update -   Update omni to the latest GitHub release"
    ))]
    /// Update omni to the latest GitHub release
    SelfUpdate(self::commands::self_update::SelfUpdate),
}

/// xterm-256 color 141 (#af87ff): the closest palette entry to the website's
/// violet accent (#a78bfa). A palette index rather than 24-bit RGB so it
/// renders on terminals without truecolor support (e.g. macOS Terminal.app).
pub const ACCENT_ANSI: u8 = 141;

/// near-cli-rs's interactive style with omni's accent color: the prompt
/// markers and the selection dot are violet instead of green. Guide lines
/// stay blue and input/help text yellow, matching the website's terminal
/// semantics.
fn render_config() -> inquire::ui::RenderConfig<'static> {
    use inquire::ui::{Color, StyleSheet, Styled};

    let accent = Color::AnsiValue(ACCENT_ANSI);
    let mut render_config = near_cli_rs::get_global_render_config();
    render_config.prompt_prefix = Styled::new("◆ ").with_fg(accent);
    render_config.answered_prompt_prefix = Styled::new("◇ ").with_fg(accent);
    render_config.highlighted_option_prefix = Styled::new(" ●").with_fg(accent);
    render_config.selected_checkbox = Styled::new("◼").with_fg(accent);
    render_config.selected_option = Some(StyleSheet::new().with_fg(Color::Grey));
    render_config
}

fn main() -> CliResult {
    inquire::set_global_render_config(render_config());

    let near_config = near_cli_rs::config::Config::get_config_toml()?;

    #[cfg(not(debug_assertions))]
    let display_env_section = false;
    #[cfg(debug_assertions)]
    let display_env_section = true;
    color_eyre::config::HookBuilder::default()
        .display_env_section(display_env_section)
        .install()?;

    let cli = match Cmd::try_parse() {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };

    let verbosity = match (cli.quiet, cli.teach_me) {
        (true, _) => Verbosity::Quiet,
        (false, true) => Verbosity::TeachMe,
        (false, false) => Verbosity::Interactive,
    };
    near_cli_rs::setup_tracing(verbosity)?;

    let global_context = near_cli_rs::GlobalContext {
        config: near_config,
        offline: false,
        verbosity,
    };

    let cli_cmd = match <Cmd as interactive_clap::FromCli>::from_cli(
        Some(cli.clone()),
        global_context.clone(),
    ) {
        interactive_clap::ResultFromCli::Ok(cli_cmd)
        | interactive_clap::ResultFromCli::Cancel(Some(cli_cmd)) => {
            eprintln!(
                "Your console command:\n{}",
                format!(
                    "{} {}",
                    std::env::args().next().as_deref().unwrap_or("./omni"),
                    shell_words::join(cli_cmd.to_cli_args())
                )
                .yellow()
            );
            Ok(Some(cli_cmd))
        }
        interactive_clap::ResultFromCli::Cancel(None) => {
            eprintln!("Goodbye!");
            Ok(None)
        }
        interactive_clap::ResultFromCli::Back => {
            unreachable!("TopLevelCommand does not have back option")
        }
        interactive_clap::ResultFromCli::Err(optional_cli_cmd, err) => {
            if let Some(cli_cmd) = optional_cli_cmd {
                eprintln!(
                    "Your console command:\n{}",
                    format!(
                        "{} {}",
                        std::env::args().next().as_deref().unwrap_or("./omni"),
                        shell_words::join(cli_cmd.to_cli_args())
                    )
                    .yellow()
                );
            }
            Err(err)
        }
    };

    cli_cmd.map(|_| ())
}
