pub mod start;
pub mod start_tui;
pub mod command;
pub mod client;

use clap::Parser;
use command::Commands;
#[derive(Parser)]
#[command(name = "pizza-factory")]
#[command(about = "Decentralized Pizza Factory")]
#[command(version = "1.0")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}
