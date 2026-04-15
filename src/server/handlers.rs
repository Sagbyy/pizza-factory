use std::collections::{HashMap, HashSet};
use std::net::TcpStream;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::network::tcp::{read_frame, write_frame};
use crate::node::NodeState;
use crate::protocol::{
    ProcessPayload, RecipeAvailability, RecipeStatus, TcpMessage, Update, from_cbor, to_cbor,
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

    let mut recipes: HashMap<String, RecipeAvailability> = state
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
                    remote_peers: vec![],
                },
            )
        })
        .collect();

    // Add recipes discovered via gossip from remote peers.
    {
        let gossip = state.gossip.read().unwrap();
        for (peer_addr, peer_info) in &gossip.peers {
            for recipe_name in &peer_info.recipes {
                recipes
                    .entry(recipe_name.clone())
                    .or_insert_with(|| RecipeAvailability {
                        local: RecipeStatus {
                            missing_actions: vec![],
                        },
                        remote_peers: vec![],
                    })
                    .remote_peers
                    .push(peer_addr.clone());
            }
        }
    }

    TcpMessage::RecipeListAnswer { recipes }
}

/// Return the canonical DSL string for a named recipe.
///
/// Used by a peer that does not hold the recipe file itself and needs to
/// retrieve it before building the initial `ProcessPayload`.
pub fn handle_get_recipe(state: &NodeState, recipe_name: &str) -> TcpMessage {
    // First check local recipes
    if let Some(recipe) = state
        .identity
        .recipes
        .iter()
        .find(|r| r.name == recipe_name)
    {
        return TcpMessage::RecipeAnswer {
            recipe: recipe.source.clone(),
        };
    }

    // If not local, find candidate peers via gossip
    let candidate_peers: Vec<String> = {
        let gossip = state.gossip.read().unwrap();
        gossip
            .peers
            .iter()
            .filter(|(_, info)| info.recipes.iter().any(|r| r == recipe_name))
            .map(|(addr, _)| addr.clone())
            .collect()
    };

    if let Some(response) = try_forward_get_recipe(recipe_name, &candidate_peers) {
        return response;
    }

    TcpMessage::Error {
        message: format!("recipe '{recipe_name}' not found locally and no peer advertised it"),
    }
}

/// Place an order for a recipe.
/// Returns an order receipt for local recipes.
/// If recipe is not local, it hints candidate peers learned via gossip.
pub fn handle_order(state: &NodeState, recipe_name: &str) -> TcpMessage {
    if state.identity.recipes.iter().any(|r| r.name == recipe_name) {
        return TcpMessage::OrderReceipt {
            order_id: crate::protocol::uuid(Uuid::new_v4()),
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

    if let Some(response) = try_forward_order(recipe_name, &candidate_peers) {
        return response;
    }

    TcpMessage::OrderDeclined {
        message: "Unknown recipe".to_string(),
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

        if let Some(response) = try_forward_payload(&payload, &candidate_peers) {
            return response;
        }

        return TcpMessage::Error {
            message: format!(
                "cannot execute action '{}' locally and forwarding failed; candidate peers: {}",
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

fn forward_to_peer(peer: &str, message: &TcpMessage) -> Result<TcpMessage, String> {
    let addr: std::net::SocketAddr = peer
        .parse()
        .map_err(|e| format!("invalid peer address {peer}: {e}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500))
        .map_err(|e| format!("connect {peer}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| format!("set read timeout: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|e| format!("set write timeout: {e}"))?;

    let bytes = to_cbor(message).map_err(|e| format!("encode request: {e}"))?;
    write_frame(&mut stream, &bytes).map_err(|e| format!("write frame: {e}"))?;

    let response_bytes = read_frame(&mut stream).map_err(|e| format!("read frame: {e}"))?;
    let response = from_cbor(&response_bytes).map_err(|e| format!("decode response: {e}"))?;
    Ok(response)
}

fn try_forward_order(recipe_name: &str, candidate_peers: &[String]) -> Option<TcpMessage> {
    let forwarded = TcpMessage::Order {
        recipe_name: recipe_name.to_string(),
    };

    for peer in candidate_peers {
        if let Ok(response) = forward_to_peer(peer, &forwarded) {
            return Some(response);
        }
    }

    None
}

fn try_forward_get_recipe(recipe_name: &str, candidate_peers: &[String]) -> Option<TcpMessage> {
    let forwarded = TcpMessage::GetRecipe {
        recipe_name: recipe_name.to_string(),
    };

    for peer in candidate_peers {
        if let Ok(response) = forward_to_peer(peer, &forwarded) {
            return Some(response);
        }
    }

    None
}

fn try_forward_payload(payload: &ProcessPayload, candidate_peers: &[String]) -> Option<TcpMessage> {
    for peer in candidate_peers {
        let mut forwarded_payload = payload.clone();
        forwarded_payload.updates.push(Update::Forward {
            to: crate::protocol::addr(peer.clone()),
            timestamp: now_micros(),
        });

        let forwarded = TcpMessage::ProcessPayload {
            payload: forwarded_payload,
        };
        if let Ok(response) = forward_to_peer(peer, &forwarded) {
            return Some(response);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::RwLock;

    use crate::node::{GossipState as NodeGossipState, Identity, NodeState, PeerInfo};
    use crate::protocol::{ActionDef, ProcessPayload, Update, Version};
    use crate::recipe::{Recipe, Step};

    fn build_state(capabilities: Vec<&str>, recipes: Vec<&str>) -> NodeState {
        let local_recipes = recipes
            .into_iter()
            .map(|name| Recipe {
                name: name.to_string(),
                steps: vec![Step::Single(ActionDef {
                    name: "MakeDough".to_string(),
                    params: HashMap::new(),
                })],
                source: format!("{name} = MakeDough"),
            })
            .collect();

        NodeState {
            identity: Identity {
                addr: "127.0.0.1:8000".to_string(),
                capabilities: capabilities.into_iter().map(str::to_string).collect(),
                recipes: local_recipes,
            },
            gossip: RwLock::new(NodeGossipState {
                peers: HashMap::new(),
                version: Version {
                    counter: 1,
                    generation: 1,
                },
            }),
        }
    }

    #[test]
    fn handle_order_returns_receipt_for_local_recipe() {
        let state = build_state(vec!["MakeDough"], vec!["Margherita"]);

        let response = handle_order(&state, "Margherita");

        assert!(matches!(response, TcpMessage::OrderReceipt { .. }));
    }

    /// Bind to an ephemeral port then immediately drop the listener.
    /// The OS will refuse new connections to that port → reliably unreachable.
    fn closed_addr() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        drop(listener);
        addr
    }

    #[test]
    fn handle_order_declines_when_recipe_unknown_or_unreachable() {
        // Case 1: no peer knows the recipe
        let state = build_state(vec!["MakeDough"], vec![]);
        let response = handle_order(&state, "UnknownPizza");
        assert!(
            matches!(response, TcpMessage::OrderDeclined { ref message } if message == "Unknown recipe"),
            "expected OrderDeclined with 'Unknown recipe', got {response:?}"
        );

        // Case 2: a peer claims to know the recipe but the port is closed (unreachable)
        let unreachable = closed_addr();
        {
            let mut gossip = state.gossip.write().unwrap();
            gossip.peers.insert(
                unreachable,
                PeerInfo {
                    capabilities: vec!["Bake".to_string()],
                    recipes: vec!["Pepperoni".to_string()],
                    version: Version {
                        counter: 2,
                        generation: 1,
                    },
                    last_seen_us: 1,
                },
            );
        }
        let response = handle_order(&state, "Pepperoni");
        assert!(
            matches!(response, TcpMessage::OrderDeclined { ref message } if message == "Unknown recipe"),
            "expected OrderDeclined with 'Unknown recipe', got {response:?}"
        );
    }

    #[test]
    fn handle_process_payload_advances_action_when_capable() {
        let state = build_state(vec!["MakeDough"], vec![]);
        let payload = ProcessPayload {
            order_id: crate::protocol::uuid(uuid::Uuid::nil()),
            order_timestamp: 1,
            delivery_host: crate::protocol::addr("127.0.0.1:9000"),
            action_index: 0,
            action_sequence: vec![ActionDef {
                name: "MakeDough".to_string(),
                params: HashMap::new(),
            }],
            content: "payload".to_string(),
            updates: vec![],
        };

        let response = handle_process_payload(&state, payload);

        match response {
            TcpMessage::CompletedOrder { result, .. } => {
                assert_eq!(result, "payload");
            }
            TcpMessage::ProcessPayload { payload } => {
                assert_eq!(payload.action_index, 1);
                assert!(matches!(
                    payload.updates.last(),
                    Some(Update::Action { .. })
                ));
            }
            _ => panic!("expected ProcessPayload or CompletedOrder response"),
        }
    }

    #[test]
    fn handle_process_payload_reports_candidate_peer_when_missing_capability() {
        let state = build_state(vec!["Bake"], vec![]);
        {
            let mut gossip = state.gossip.write().unwrap();
            gossip.peers.insert(
                "127.0.0.1:8003".to_string(),
                PeerInfo {
                    capabilities: vec!["MakeDough".to_string()],
                    recipes: vec![],
                    version: Version {
                        counter: 3,
                        generation: 1,
                    },
                    last_seen_us: 1,
                },
            );
        }

        let payload = ProcessPayload {
            order_id: crate::protocol::uuid(uuid::Uuid::nil()),
            order_timestamp: 1,
            delivery_host: crate::protocol::addr("127.0.0.1:9000"),
            action_index: 0,
            action_sequence: vec![ActionDef {
                name: "MakeDough".to_string(),
                params: HashMap::new(),
            }],
            content: String::new(),
            updates: vec![],
        };

        let response = handle_process_payload(&state, payload);

        match response {
            TcpMessage::Error { message } => {
                assert!(message.contains("cannot execute action"));
                assert!(message.contains("127.0.0.1:8003"));
            }
            _ => panic!("expected Error response"),
        }
    }
}
