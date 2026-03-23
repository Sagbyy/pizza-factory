mod cli;
mod network;
mod protocol;

use clap::Parser;
use cli::Cli;

fn main() {
    let cli = Cli::parse();

    println!("{:?}", cli.command);
}
