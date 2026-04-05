#![allow(dead_code, unused_imports)]

mod cli;
mod network;
mod node;
mod protocol;
mod recipe;
mod server;

use std::sync::Arc;

use clap::Parser;
use cli::Cli;
use cli::command::Commands;
use cli::client::ClientCommands;

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start(args) => {
            let state = match node::NodeState::new(&args) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("PizzaFactory failed: {e}");
                    if let Some(cause) = std::error::Error::source(&e) {
                        eprintln!("\nCaused by:\n    {cause}");
                    }
                    std::process::exit(1);
                }
            };

            let tcp_handle = match server::tcp::start(Arc::clone(&state)) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("PizzaFactory failed: Failed to start TCP server on {}: {e}", args.host);
                    std::process::exit(1);
                }
            };

            println!("Starting server on {}...", args.host);
            tcp_handle.join().unwrap();
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
