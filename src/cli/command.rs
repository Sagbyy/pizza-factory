use clap::Subcommand;
use crate::cli::start::StartArgs;
use crate::cli::start_tui::StartTuiArgs;
use crate::cli::client::ClientArgs;

#[derive(Subcommand, Debug)]
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