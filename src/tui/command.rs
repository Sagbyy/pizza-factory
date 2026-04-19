use std::sync::Arc;

use clap::{CommandFactory, Parser, Subcommand};

use crate::cli::client_impl::format_recipe_row;
use crate::node::NodeState;
use crate::protocol::TcpMessage;
use crate::server::handlers::handle_list_recipes;

#[derive(Parser)]
#[command(
    name = "",
    disable_help_flag = true,
    disable_help_subcommand = true,
    disable_version_flag = true
)]
struct TuiCli {
    #[command(subcommand)]
    command: TuiCommand,
}

#[derive(Subcommand)]
enum TuiCommand {
    /// Show available commands
    Help,
    /// List capabilities exposed by this node
    ListCapabilities,
    /// List recipes available in the cluster
    ListRecipes,
}

pub fn execute(input: &str, state: &Arc<NodeState>) {
    let tokens = std::iter::once("tui").chain(input.split_whitespace());

    match TuiCli::try_parse_from(tokens) {
        Ok(cli) => match cli.command {
            TuiCommand::Help => {
                for line in TuiCli::command().render_help().to_string().lines() {
                    if !line.starts_with("Usage:") {
                        log::info!(target: "command", "{line}");
                    }
                }
            }
            TuiCommand::ListCapabilities => {
                let caps = &state.identity.capabilities;
                if caps.is_empty() {
                    log::info!(target: "command", "No capabilities");
                } else {
                    log::info!(target: "command", "Capabilities: {}", caps.join(", "));
                }
            }
            TuiCommand::ListRecipes => match handle_list_recipes(state) {
                TcpMessage::RecipeListAnswer { recipes } => {
                    if recipes.is_empty() {
                        log::info!(target: "command", "No recipes available");
                        return;
                    }
                    log::info!(target: "command", "Available recipes:");
                    let mut names: Vec<String> = recipes.keys().cloned().collect();
                    names.sort();
                    for name in names {
                        let row = format_recipe_row(&name, recipes.get(&name).unwrap());
                        log::info!(target: "command", "{row}");
                    }
                }
                _ => log::warn!(target: "command", "Unexpected response"),
            },
        },
        Err(e) => {
            for line in e.render().to_string().lines() {
                log::warn!(target: "command", "{line}");
            }
        }
    }
}
