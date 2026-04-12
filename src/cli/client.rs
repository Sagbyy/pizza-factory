use clap::{Args, Subcommand};

#[derive(Args, Debug)]
/// Arguments for the external TCP client mode.
pub struct ClientArgs {
    /// Target peer TCP address (`host:port`).
    #[arg(long, value_name = "PEER:PORT", help = "Peer TCP address (ip:port)")]
    pub peer: String,
    /// Client subcommand to execute.
    #[command(subcommand)]
    pub command: ClientCommands,
}

#[derive(Subcommand, Debug)]
/// Supported client operations against a remote node.
pub enum ClientCommands {
    #[command(about = "Place an order for a recipe on a peer")]
    Order {
        #[arg(value_name = "RECIPE", help = "Recipe name")]
        recipe: String,
    },
    #[command(about = "List available recipes from a peer")]
    ListRecipes,
    #[command(about = "Get a specific recipe from a peer")]
    GetRecipe {
        #[arg(value_name = "RECIPE", help = "Recipe name")]
        recipe: String,
    },
}
