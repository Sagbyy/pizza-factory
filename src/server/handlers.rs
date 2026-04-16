use std::collections::{HashMap, HashSet};
use std::net::TcpStream;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::network::tcp::{read_frame, write_frame};
use crate::node::NodeState;
use crate::protocol::{
    OrderResult, ProcessPayload, RecipeAvailability, RecipeStatus, TcpMessage, Update, from_cbor,
    to_cbor,
};
use crate::recipe::{flatten_recipe, parse_recipes};
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
///
/// Writes `OrderReceipt` immediately to `stream` (frame 1), then fetches the
/// recipe, builds the initial `ProcessPayload`, and drives execution to
/// completion. Returns `CompletedOrder` or `Error` as frame 2 (written by the
/// caller). Returns `OrderDeclined` without touching `stream` when the recipe
/// is unknown.
pub fn handle_order(state: &NodeState, recipe_name: &str, stream: &mut TcpStream) -> TcpMessage {
    // Check availability: local recipe file or a peer that advertised it.
    let is_local = state.identity.recipes.iter().any(|r| r.name == recipe_name);
    let peer_knows = {
        let gossip = state.gossip.read().unwrap();
        gossip
            .peers
            .values()
            .any(|info| info.recipes.iter().any(|r| r == recipe_name))
    };

    if !is_local && !peer_knows {
        return TcpMessage::OrderDeclined {
            message: "Unknown recipe".to_string(),
        };
    }

    // Acknowledge immediately — client gets the UUID before any work starts.
    let order_id = Uuid::new_v4();
    let receipt = TcpMessage::OrderReceipt {
        order_id: crate::protocol::uuid(order_id),
    };
    match to_cbor(&receipt).map_err(|e| format!("encode receipt: {e}")) {
        Ok(bytes) => {
            if let Err(e) = write_frame(stream, &bytes) {
                return TcpMessage::Error {
                    message: format!("send receipt: {e}"),
                };
            }
        }
        Err(e) => return TcpMessage::Error { message: e },
    }

    // Fetch DSL — local first, then from the peer that advertised it.
    let dsl = if is_local {
        state
            .identity
            .recipes
            .iter()
            .find(|r| r.name == recipe_name)
            .map(|r| r.source.clone())
            .unwrap_or_default()
    } else {
        match handle_get_recipe(state, recipe_name) {
            TcpMessage::RecipeAnswer { recipe } => recipe,
            TcpMessage::Error { message } => {
                return TcpMessage::Error {
                    message: format!("fetch recipe: {message}"),
                }
            }
            other => {
                return TcpMessage::Error {
                    message: format!("unexpected get_recipe response: {other:?}"),
                }
            }
        }
    };

    // Parse and flatten the recipe into a linear action sequence.
    let recipes = match parse_recipes(&dsl) {
        Ok(r) => r,
        Err(e) => {
            return TcpMessage::Error {
                message: format!("parse recipe: {e}"),
            }
        }
    };
    let recipe = match recipes.into_iter().find(|r| r.name == recipe_name) {
        Some(r) => r,
        None => {
            return TcpMessage::Error {
                message: format!("recipe '{recipe_name}' not found after fetch"),
            }
        }
    };
    let action_sequence = flatten_recipe(&recipe);

    // Build the initial payload and drive execution.
    let payload = ProcessPayload {
        order_id: crate::protocol::uuid(order_id),
        order_timestamp: now_micros(),
        delivery_host: crate::protocol::addr(state.identity.addr.clone()),
        action_index: 0,
        action_sequence,
        content: String::new(),
        updates: vec![],
    };

    handle_process_payload(state, recipe_name, payload)
}

/// Execute actions in a payload loop until this node can no longer proceed,
/// then forward to a peer or deliver the completed order.
///
/// Each iteration executes one action this node is capable of, updates
/// `content` and `updates`, and advances `action_index`. When an action
/// exceeds local capabilities the payload is forwarded to a peer that has it.
/// When all actions are done a `Deliver` update is appended and `CompletedOrder`
/// is returned.
///
/// `recipe_name` is used to label the `CompletedOrder`. Pass `"unknown"` when
/// the name is not available (e.g. inter-agent `ProcessPayload` hops).
pub fn handle_process_payload(state: &NodeState, recipe_name: &str, mut payload: ProcessPayload) -> TcpMessage {
    loop {
        let idx = payload.action_index as usize;

        if idx >= payload.action_sequence.len() {
            payload.updates.push(Update::Deliver { timestamp: now_micros() });
            return TcpMessage::CompletedOrder {
                recipe_name: recipe_name.to_string(),
                result: build_result(&payload),
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

        payload.content = apply_action(&action, &payload.content);
        payload.updates.push(Update::Action {
            action,
            timestamp: now_micros(),
        });
        payload.action_index += 1;
    }
}

/// Map an executed action to the line it appends to the pizza content string.
fn apply_action(action: &crate::protocol::ActionDef, current_content: &str) -> String {
    let line = match action.name.as_str() {
        "MakeDough" => "Dough: ready\n".to_string(),
        "AddBase" => {
            let base = action.params.get("base_type").map(String::as_str).unwrap_or("unknown");
            format!("Dough + Base({base}): ready\n")
        }
        "AddCheese" => {
            let amount = action.params.get("amount").map(String::as_str).unwrap_or("1");
            format!("Cheese x{amount}\n")
        }
        "AddPepperoni" => {
            let slices = action.params.get("slices").map(String::as_str).unwrap_or("0");
            format!("Pepperoni slices x{slices}\n")
        }
        "Bake" => {
            let duration = action.params.get("duration").map(String::as_str).unwrap_or("0");
            format!("Baked({duration})\n")
        }
        "AddOliveOil"  => "OliveOil: added\n".to_string(),
        "AddMushrooms" => "Mushrooms: added\n".to_string(),
        "AddBasil"     => "Basil: added\n".to_string(),
        "AddGarlic"    => "Garlic: added\n".to_string(),
        "AddOregano"   => "Oregano: added\n".to_string(),
        other          => format!("{other}: done\n"),
    };
    format!("{current_content}{line}")
}

/// Serialize the completed payload into the JSON string stored in `CompletedOrder.result`.
fn build_result(payload: &ProcessPayload) -> String {
    let order_result = OrderResult {
        order_id: payload.order_id.clone(),
        order_timestamp: payload.order_timestamp,
        content: payload.content.clone(),
        updates: payload.updates.clone(),
    };
    serde_json::to_string_pretty(&order_result).unwrap_or_else(|_| payload.content.clone())
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn forward_to_peer(peer: &str, message: &TcpMessage) -> Result<TcpMessage, String> {
    let mut stream = TcpStream::connect(peer).map_err(|e| format!("connect {peer}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| format!("set read timeout: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| format!("set write timeout: {e}"))?;

    let bytes = to_cbor(message).map_err(|e| format!("encode request: {e}"))?;
    write_frame(&mut stream, &bytes).map_err(|e| format!("write frame: {e}"))?;

    let response_bytes = read_frame(&mut stream).map_err(|e| format!("read frame: {e}"))?;
    let response = from_cbor(&response_bytes).map_err(|e| format!("decode response: {e}"))?;
    Ok(response)
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
    use crate::protocol::{ActionDef, ProcessPayload, Version};
    use crate::recipe::{Recipe, Step};

    /// Create a connected loopback stream pair for tests that need a TcpStream.
    /// Returns (client_side, server_side).
    fn make_stream_pair() -> (TcpStream, TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

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
    fn handle_order_sends_receipt_then_returns_completed_order() {
        let state = build_state(vec!["MakeDough"], vec!["Margherita"]);
        let (mut client, mut server) = make_stream_pair();

        let response = handle_order(&state, "Margherita", &mut server);

        // Frame 1: OrderReceipt written directly to the stream.
        let receipt_bytes = read_frame(&mut client).unwrap();
        let receipt: TcpMessage = from_cbor(&receipt_bytes).unwrap();
        assert!(
            matches!(receipt, TcpMessage::OrderReceipt { .. }),
            "expected OrderReceipt as frame 1, got {receipt:?}"
        );

        // Return value: CompletedOrder (frame 2, written by handle_connection).
        assert!(
            matches!(response, TcpMessage::CompletedOrder { .. }),
            "expected CompletedOrder as frame 2, got {response:?}"
        );
    }

    #[test]
    fn handle_order_declines_when_recipe_unknown_or_unreachable() {
        let state = build_state(vec!["MakeDough"], vec![]);
        let (mut _client, mut server) = make_stream_pair();

        // Case 1: no peer knows the recipe — OrderDeclined, stream untouched.
        let response = handle_order(&state, "UnknownPizza", &mut server);
        assert!(
            matches!(response, TcpMessage::OrderDeclined { ref message } if message == "Unknown recipe"),
            "expected OrderDeclined with 'Unknown recipe', got {response:?}"
        );

        // Case 2: a peer claims to know the recipe but is unreachable.
        // Port 19999 is used so tests never hit a real running agent.
        {
            let mut gossip = state.gossip.write().unwrap();
            gossip.peers.insert(
                "127.0.0.1:19999".to_string(),
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
        // Peer is known → receipt is sent → get_recipe fails → Error as frame 2.
        let (mut client2, mut server2) = make_stream_pair();
        let response = handle_order(&state, "Pepperoni", &mut server2);
        // Receipt was written — consume it so the stream stays clean.
        let _ = read_frame(&mut client2);
        assert!(
            matches!(response, TcpMessage::Error { .. }),
            "expected Error when peer unreachable after receipt, got {response:?}"
        );
    }

    #[test]
    fn handle_process_payload_executes_all_capable_actions_and_delivers() {
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
            content: String::new(),
            updates: vec![],
        };

        let response = handle_process_payload(&state, "TestRecipe", payload);

        match response {
            TcpMessage::CompletedOrder { result, recipe_name, .. } => {
                assert_eq!(recipe_name, "TestRecipe");
                // result is a JSON string — verify it contains the expected content
                assert!(result.contains("Dough: ready"), "content missing from result: {result}");
                assert!(result.contains("order_id"), "order_id missing from result: {result}");
                assert!(result.contains("updates"), "updates missing from result: {result}");
            }
            other => panic!("expected CompletedOrder, got {other:?}"),
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

        let response = handle_process_payload(&state, "TestRecipe", payload);

        match response {
            TcpMessage::Error { message } => {
                assert!(message.contains("cannot execute action"));
                assert!(message.contains("127.0.0.1:8003"));
            }
            _ => panic!("expected Error response"),
        }
    }
}
