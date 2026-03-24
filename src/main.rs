#![allow(dead_code, unused_imports)]

mod cli;
mod network;
mod protocol;
mod recipe;

use clap::Parser;
use cli::Cli;
use cli::command::Commands;
use cli::client::ClientCommands;

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start(args) => {
            println!("Starting server on {:?}...", args);
        }
        Commands::StartTui(args) => {
            println!("Starting TUI server on {:?}...", args);
        }
        Commands::ListCapabilities => {
            println!("Listing capabilities...");
        }
        Commands::Client(args) => match args.command {
            ClientCommands::Order { recipe } => {
                println!("Ordering recipe '{}' from {}...", recipe, args.peer);
            }
            ClientCommands::ListRecipes => {
                println!("Listing recipes from {}...", args.peer);
            }
            ClientCommands::GetRecipe { recipe } => {
                println!("Getting recipe '{}' from {}...", recipe, args.peer);
            }
        },
    }
}
