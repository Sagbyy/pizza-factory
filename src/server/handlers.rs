use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::node::NodeState;
use crate::protocol::{
    ProcessPayload, RecipeAvailability, RecipeStatus, Tagged, TcpMessage, Update,
};
use crate::recipe::flatten_recipe;
use uuid::Uuid;

/// Build the recipe list answer for a `ListRecipes` command.
///
/// For each recipe the node knows, computes which actions are required but
/// not covered by this node's own capabilities (`missing_actions`).
/// Duplicates in the action sequence are collapsed — each action name appears
/// at most once in `missing_actions`, in recipe order.
pub fn handle_list_recipes(state: &NodeState) -> TcpMessage {
    let capabilities: HashSet<&str> = state
        .identity
        .capabilities
        .iter()
        .map(String::as_str)
        .collect();

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
                    local: RecipeStatus {
                        missing_actions: missing,
                    },
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
    match state
        .identity
        .recipes
        .iter()
        .find(|r| r.name == recipe_name)
    {
        Some(recipe) => TcpMessage::RecipeAnswer {
            recipe: recipe.source.clone(),
        },
        None => TcpMessage::Error {
            message: format!("recipe '{recipe_name}' not found"),
        },
    }
}

/// Place an order for a recipe.
/// Returns an order receipt for local recipes.
/// If recipe is not local, it hints candidate peers learned via gossip.
pub fn handle_order(state: &NodeState, recipe_name: &str) -> TcpMessage {
    if state.identity.recipes.iter().any(|r| r.name == recipe_name) {
        return TcpMessage::OrderReceipt {
            order_id: Tagged::uuid(Uuid::new_v4()),
        };
    }

    let candidate_peers: Vec<String> = {
        let gossip = state.gossip.read().unwrap();
        gossip
            .peers
            .iter()
            .filter(|(_, info)| info.recipes.iter().any(|r| r == recipe_name))
            .map(|(addr, _)| addr.clone())
            .collect()
    };

    if candidate_peers.is_empty() {
        TcpMessage::Error {
            message: format!(
                "recipe '{recipe_name}' not found locally and no peer advertised it yet"
            ),
        }
    } else {
        TcpMessage::Error {
            message: format!(
                "recipe '{recipe_name}' not local; candidate peers: {}",
                candidate_peers.join(", ")
            ),
        }
    }
}

/// Execute the next action in a payload and forward or deliver.
/// This local step validates capability and advances the action index.
pub fn handle_process_payload(state: &NodeState, mut payload: ProcessPayload) -> TcpMessage {
    let idx = payload.action_index as usize;
    if idx >= payload.action_sequence.len() {
        return TcpMessage::CompletedOrder {
            recipe_name: "unknown".into(),
            result: payload.content,
        };
    }

    let action = payload.action_sequence[idx].clone();
    let can_execute = state
        .identity
        .capabilities
        .iter()
        .any(|cap| cap == &action.name);

    if !can_execute {
        let candidate_peers: Vec<String> = {
            let gossip = state.gossip.read().unwrap();
            gossip
                .peers
                .iter()
                .filter(|(_, info)| info.capabilities.iter().any(|cap| cap == &action.name))
                .map(|(addr, _)| addr.clone())
                .collect()
        };

        return TcpMessage::Error {
            message: format!(
                "cannot execute action '{}' locally; candidate peers: {}",
                action.name,
                if candidate_peers.is_empty() {
                    "none".to_string()
                } else {
                    candidate_peers.join(", ")
                }
            ),
        };
    }

    payload.updates.push(Update::Action {
        action,
        timestamp: now_micros(),
    });
    payload.action_index += 1;

    if payload.action_index as usize >= payload.action_sequence.len() {
        return TcpMessage::CompletedOrder {
            recipe_name: "unknown".into(),
            result: payload.content,
        };
    }

    TcpMessage::ProcessPayload { payload }
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
