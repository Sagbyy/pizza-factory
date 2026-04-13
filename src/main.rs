//! Pizza Factory node executable.
//!
//! This binary starts either a server node (`start` / `start-tui`) or a client
//! command (`client`). The server path runs TCP request handling and UDP gossip
//! side by side so nodes can discover peers and forward recipe operations.

mod cli;
mod network;
mod node;
mod protocol;
mod recipe;
mod server;
mod tui;

use std::sync::Arc;

use clap::Parser;
use cli::Cli;
use cli::client::ClientCommands;
use cli::command::Commands;
use network::udp::run_gossip_service_shared;
use std::net::UdpSocket;
use std::thread;

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
                    eprintln!(
                        "PizzaFactory failed: Failed to start TCP server on {}: {e}",
                        args.host
                    );
                    std::process::exit(1);
                }
            };

            let socket = UdpSocket::bind(&args.host).expect("failed to bind UDP socket");
            socket
                .set_read_timeout(Some(std::time::Duration::from_millis(500)))
                .expect("failed to set UDP socket timeout");
            let peers = args.peers.clone();
            let udp_state = Arc::clone(&state);

            let _udp_handle = thread::spawn(move || {
                println!(
                    "Starting UDP gossip service on {}...",
                    udp_state.identity.addr
                );

                if let Err(e) = run_gossip_service_shared(&socket, udp_state, &peers) {
                    eprintln!("UDP gossip service stopped unexpectedly: {e}");
                }
            });

            println!("Starting server on {}...", args.host);
            tcp_handle.join().unwrap();
        }
        Commands::StartTui(args) => {
            println!("Starting TUI server on {:?}...", args);
            match ratatui::run(tui::app::start_tui) {
                Ok(_) => println!("TUI server stopped."),
                Err(e) => {
                    eprintln!("TUI server error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::ListCapabilities => {
            println!("Listing capabilities...");
        }
        Commands::Client(args) => match args.command {
            ClientCommands::Order { recipe } => {
                if let Err(e) = cli::client_impl::client_order(&args.peer, &recipe) {
                    eprintln!("Client error: {}", e);
                    std::process::exit(1);
                }
            }
            ClientCommands::ListRecipes => {
                if let Err(e) = cli::client_impl::client_list_recipes(&args.peer) {
                    eprintln!("Client error: {}", e);
                    std::process::exit(1);
                }
            }
            ClientCommands::GetRecipe { recipe } => {
                if let Err(e) = cli::client_impl::client_get_recipe(&args.peer, &recipe) {
                    eprintln!("Client error: {}", e);
                    std::process::exit(1);
                }
            }
        },
    }
}
