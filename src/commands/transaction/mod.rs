use strum::{EnumDiscriminants, EnumIter, EnumMessage};

pub mod broadcast;
pub mod construct;

#[derive(Debug, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
pub struct TransactionCommands {
    #[interactive_clap(subcommand)]
    transaction_actions: TransactionActions,
}

#[derive(Debug, EnumDiscriminants, Clone, interactive_clap::InteractiveClap)]
#[interactive_clap(context = near_cli_rs::GlobalContext)]
#[strum_discriminants(derive(EnumMessage, EnumIter))]
/// Select the action:
pub enum TransactionActions {
    #[strum_discriminants(strum(
        message = "construct   -   Construct a transaction for another chain and sign it with MPC (directly or via a DAO proposal)"
    ))]
    /// Construct a transaction for another chain and sign it with MPC (directly or via a DAO proposal)
    Construct(self::construct::Construct),
    #[strum_discriminants(strum(
        message = "broadcast   -   Assemble a signed transaction from a NEAR tx (approved proposal or direct sign) and broadcast it"
    ))]
    /// Assemble a signed transaction from a NEAR tx (approved proposal or direct sign) and broadcast it
    Broadcast(self::broadcast::Broadcast),
}
