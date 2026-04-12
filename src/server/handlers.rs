use std::collections::{HashMap, HashSet};

use crate::node::NodeState;
use crate::protocol::{ProcessPayload, RecipeAvailability, RecipeStatus, TcpMessage};
use crate::recipe::flatten_recipe;

/// Build the recipe list answer for a `ListRecipes` command.
///
/// For each recipe the node knows, computes which actions are required but
/// not covered by this node's own capabilities (`missing_actions`).
/// Duplicates in the action sequence are collapsed — each action name appears
/// at most once in `missing_actions`, in recipe order.
pub fn handle_list_recipes(state: &NodeState) -> TcpMessage {
    let capabilities: HashSet<&str> =
        state.identity.capabilities.iter().map(String::as_str).collect();

    let recipes: HashMap<String, RecipeAvailability> = state
        .identity
        .recipes
        .iter()
        .map(|recipe| {
            let mut seen: HashSet<String> = HashSet::new();
            let missing: Vec<String> = flatten_recipe(recipe)
                .into_iter()
                .map(|a| a.name)
                .filter(|name| !capabilities.contains(name.as_str()))
                .filter(|name| seen.insert(name.clone()))
                .collect();

            (
                recipe.name.clone(),
                RecipeAvailability {
                    local: RecipeStatus { missing_actions: missing },
                },
            )
        })
        .collect();

    TcpMessage::RecipeListAnswer { recipes }
}

/// Return the canonical DSL string for a named recipe.
///
/// Used by a peer that does not hold the recipe file itself and needs to
/// retrieve it before building the initial `ProcessPayload`.
pub fn handle_get_recipe(state: &NodeState, recipe_name: &str) -> TcpMessage {
    match state.identity.recipes.iter().find(|r| r.name == recipe_name) {
        Some(recipe) => TcpMessage::RecipeAnswer { recipe: recipe.source.clone() },
        None => TcpMessage::Error { message: format!("recipe '{recipe_name}' not found") },
    }
}

/// Place an order for a recipe.
/// Full routing logic (get_recipe + process_payload forwarding) is not yet implemented.
pub fn handle_order(_state: &NodeState, _recipe_name: &str) -> TcpMessage {
    TcpMessage::Error { message: "order not yet implemented".into() }
}

/// Execute the next action in a payload and forward or deliver.
/// Execution and forwarding logic is not yet implemented.
pub fn handle_process_payload(_state: &NodeState, _payload: ProcessPayload) -> TcpMessage {
    TcpMessage::Error { message: "process_payload not yet implemented".into() }
}
