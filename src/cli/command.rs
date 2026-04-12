use crate::cli::client::ClientArgs;
use crate::cli::start::StartArgs;
use crate::cli::start_tui::StartTuiArgs;
use clap::Subcommand;

#[derive(Subcommand, Debug)]
/// Top-level commands exposed by the Pizza Factory CLI.
pub enum Commands {
    #[command(about = "Start server as a console service")]
    Start(StartArgs),
    #[command(about = "Show capabilities exposed by this node")]
    ListCapabilities,
    #[command(about = "Start server with an interactive interface")]
    StartTui(StartTuiArgs),
    #[command(about = "External client to interact with the nodes")]
    Client(ClientArgs),
}
