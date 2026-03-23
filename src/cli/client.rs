use clap::{Args, Subcommand};

#[derive(Args, Debug)]
pub struct ClientArgs {   
    #[arg(long, value_name = "PEER:PORT", help = "Peer TCP address (ip:port)")]
    pub peer: String,
    #[command(subcommand)]
    pub command: ClientCommands,
}

#[derive(Subcommand, Debug)]
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
  }
}