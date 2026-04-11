#![allow(dead_code, unused_imports)]

mod cli;
mod network;
mod protocol;
mod recipe;

use clap::Parser;
use cli::Cli;
use cli::client::ClientCommands;
use cli::command::Commands;
use network::udp::{GossipState, run_gossip_service};
use recipe::parse_recipes;
use std::fs;
use std::net::UdpSocket;

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start(args) => {
            println!("Starting UDP gossip service on {}...", args.host);

            let recipes = if let Some(path) = &args.recipes_file {
                let content = fs::read_to_string(path).expect("failed to read recipes file");
                parse_recipes(&content)
                    .expect("failed to parse recipes file")
                    .into_iter()
                    .map(|recipe| recipe.name)
                    .collect()
            } else {
                Vec::new()
            };

            let socket = UdpSocket::bind(&args.host).expect("failed to bind UDP socket");
            let mut state = GossipState::new(args.host.clone(), args.capabilities.clone(), recipes);

            run_gossip_service(&socket, &mut state, &args.peers)
                .expect("UDP gossip service stopped unexpectedly");
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
