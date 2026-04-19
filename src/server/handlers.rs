use std::collections::{HashMap, HashSet};
use std::net::TcpStream;
use std::sync::mpsc;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::network::tcp::{read_frame, write_frame};
use crate::node::NodeState;
use crate::protocol::{
    OrderResult, ProcessPayload, RecipeAvailability, RecipeStatus, RemoteRecipeStatus, TcpMessage,
    Update, addr, from_cbor, to_cbor, uuid,
};
use crate::recipe::{flatten_recipe, parse_recipes};
use uuid::Uuid;

/// Build the recipe list answer for a `ListRecipes` command.
///
/// For each recipe the node knows, computes which actions are required but
/// not covered by known cluster capabilities (`missing_actions`).
/// Duplicates in the action sequence are collapsed — each action name appears
/// at most once in `missing_actions`, in recipe order.
pub fn handle_list_recipes(state: &NodeState) -> TcpMessage {
    let mut capabilities: HashSet<String> = state.identity.capabilities.iter().cloned().collect();
    {
        let gossip = match state.gossip.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for peer_info in gossip.peers.values() {
            capabilities.extend(peer_info.capabilities.iter().cloned());
        }
    }

    let mut recipes: HashMap<String, RecipeAvailability> = state
        .identity
        .recipes
        .iter()
        .map(|recipe| {
            let mut seen: HashSet<String> = HashSet::new();
            let missing: Vec<String> = flatten_recipe(recipe)
                .into_iter()
                .map(|a| a.name)
                .filter(|name| !capabilities.contains(name))
                .filter(|name| seen.insert(name.clone()))
                .collect();

            (
                recipe.name.clone(),
                RecipeAvailability::Local {
                    local: RecipeStatus {
                        missing_actions: missing,
                    },
                },
            )
        })
        .collect();

    // Add recipes discovered via gossip from remote peers using a deterministic host.
    let mut remote_hosts: HashMap<String, String> = HashMap::new();
    {
        let gossip = match state.gossip.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for (peer_addr, peer_info) in &gossip.peers {
            for recipe_name in &peer_info.recipes {
                remote_hosts
                    .entry(recipe_name.clone())
                    .and_modify(|current_host| {
                        if peer_addr < current_host {
                            *current_host = peer_addr.clone();
                        }
                    })
                    .or_insert_with(|| peer_addr.clone());
            }
        }
    }
    for (recipe_name, host) in remote_hosts {
        recipes
            .entry(recipe_name)
            .or_insert_with(|| RecipeAvailability::Remote {
                remote: RemoteRecipeStatus { host: addr(host) },
            });
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
        let gossip = match state.gossip.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
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
        let gossip = match state.gossip.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
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
    let order_id_str = order_id.to_string();
    let receipt = TcpMessage::OrderReceipt {
        order_id: uuid(order_id),
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

    // Register a channel so handle_deliver can signal us when done.
    let (tx, rx) = mpsc::sync_channel(1);
    {
        let mut pending_orders = match state.pending_orders.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        pending_orders.insert(order_id_str.clone(), tx);
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
                };
            }
            other => {
                return TcpMessage::Error {
                    message: format!("unexpected get_recipe response: {other:?}"),
                };
            }
        }
    };

    // Parse and flatten the recipe into a linear action sequence.
    let recipes = match parse_recipes(&dsl) {
        Ok(r) => r,
        Err(e) => {
            return TcpMessage::Error {
                message: format!("parse recipe: {e}"),
            };
        }
    };
    let recipe = match recipes.into_iter().find(|r| r.name == recipe_name) {
        Some(r) => r,
        None => {
            return TcpMessage::Error {
                message: format!("recipe '{recipe_name}' not found after fetch"),
            };
        }
    };
    let action_sequence = flatten_recipe(&recipe);

    log::info!(target: "orders",
        "Received Order recipe={} order_id={}",
        recipe_name,
        order_id_str
    );

    // Build the initial payload and drive execution.
    let payload = ProcessPayload {
        order_id: uuid(order_id),
        order_timestamp: now_micros(),
        delivery_host: addr(state.identity.addr.clone()),
        action_index: 0,
        action_sequence,
        content: String::new(),
        updates: vec![],
    };

    // Drive execution — on success this sends a `deliver` to delivery_host and returns
    // a placeholder CompletedOrder with an empty result string.
    // On failure (missing capability, forward error) it either returns Error /
    // OrderDeclined immediately, or sends a deliver error and returns a placeholder
    // CompletedOrder to keep waiting for channel delivery.
    let drive_result = handle_process_payload(state, recipe_name, payload);

    // A placeholder CompletedOrder with an empty result means the deliver was sent
    // successfully and handle_deliver will signal `rx`.
    // Any other return value means execution failed before reaching the deliver step —
    // clean up and propagate the error immediately without waiting on the channel.
    // This prevents accepting a partial `deliver` that the reference binary may have
    // sent with incomplete content when a step failed mid-chain.
    let deliver_sent =
        matches!(&drive_result, TcpMessage::CompletedOrder { result, .. } if result.is_empty());

    if !deliver_sent {
        let mut pending_orders = match state.pending_orders.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        pending_orders.remove(&order_id_str);
        return drive_result;
    }

    // Block until deliver arrives (or timeout).
    let result = rx
        .recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| TcpMessage::Error {
            message: "order timed out waiting for deliver".to_string(),
        });

    {
        let mut pending_orders = match state.pending_orders.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        pending_orders.remove(&order_id_str);
    }

    // Stamp the correct recipe_name (handle_deliver leaves it empty).
    match result {
        TcpMessage::CompletedOrder { result, .. } => TcpMessage::CompletedOrder {
            recipe_name: recipe_name.to_string(),
            result,
        },
        TcpMessage::FailedOrder { error, .. } => TcpMessage::FailedOrder {
            recipe_name: recipe_name.to_string(),
            error,
        },
        other => other,
    }
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
pub fn handle_process_payload(
    state: &NodeState,
    recipe_name: &str,
    mut payload: ProcessPayload,
) -> TcpMessage {
    let order_id = payload.order_id.0.clone();
    log::info!(target: "orders",
        "Received ProcessPayload order_id={} action_index={}",
        order_id,
        payload.action_index
    );

    loop {
        let idx = payload.action_index as usize;

        if idx >= payload.action_sequence.len() {
            // All actions done. Append Forward-to-delivery_host (as seen in captures),
            // then send the `deliver` message to delivery_host via a new TCP connection.
            let delivery_host = payload.delivery_host.0.clone();
            payload.updates.push(Update::Forward {
                to: addr(delivery_host.clone()),
                timestamp: now_micros(),
            });

            let deliver_msg = TcpMessage::Deliver {
                payload,
                error: None,
            };

            // Fire-and-forget: no response is expected from the delivery_host.
            return match send_deliver(&delivery_host, &deliver_msg) {
                Ok(()) => TcpMessage::CompletedOrder {
                    // Placeholder — handle_order ignores this; real result comes via channel.
                    recipe_name: recipe_name.to_string(),
                    result: String::new(),
                },
                Err(e) => TcpMessage::Error {
                    message: format!("deliver to {delivery_host} failed: {e}"),
                },
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
                let gossip = match state.gossip.read() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                gossip
                    .peers
                    .iter()
                    .filter(|(_, info)| info.capabilities.iter().any(|cap| cap == &action.name))
                    .map(|(addr_str, _)| addr_str.clone())
                    .collect()
            };

            log::info!(target: "actions",
                "Forwarding executing action {} to {}",
                action.name,
                candidate_peers
                    .first()
                    .map(String::as_str)
                    .unwrap_or("none")
            );

            if let Some(response) = try_forward_payload(&payload, &candidate_peers) {
                return response;
            }

            let delivery_host = payload.delivery_host.0.clone();
            let unavailable_error = format!("Action {} not available", action.name);
            let candidate_peers_msg = if candidate_peers.is_empty() {
                "none".to_string()
            } else {
                candidate_peers.join(", ")
            };

            log::error!(target: "actions",
                "cannot execute action '{}' locally and forwarding failed; candidate peers: {}",
                action.name,
                candidate_peers_msg
            );

            let deliver_msg = TcpMessage::Deliver {
                payload,
                error: Some(unavailable_error.clone()),
            };

            return match send_deliver(&delivery_host, &deliver_msg) {
                Ok(()) => TcpMessage::CompletedOrder {
                    // Placeholder — real result will arrive through handle_deliver.
                    recipe_name: recipe_name.to_string(),
                    result: String::new(),
                },
                Err(e) => TcpMessage::Error {
                    message: format!(
                        "deliver failure '{}' to {delivery_host} failed: {e}",
                        unavailable_error
                    ),
                },
            };
        }

        log::info!(target: "actions",
            "Executing action {} locally order_id={}",
            action.name,
            order_id
        );
        payload.content = apply_action(&action, &payload.content);
        payload.updates.push(Update::Action {
            action,
            timestamp: now_micros(),
        });
        payload.action_index += 1;
    }
}

/// Handle an incoming `deliver` message.
///
/// Called by the dispatch loop when a peer (or this node itself) sends the final
/// payload after completing all actions.  The function:
/// 1. Appends a `Deliver` update with the current timestamp.
/// 2. Builds the `CompletedOrder` result.
/// 3. Signals the `handle_order` thread that is blocking on the channel for
///    this `order_id` — which then writes `CompletedOrder` to the client stream.
pub fn handle_deliver(
    state: &NodeState,
    mut payload: ProcessPayload,
    _error: Option<String>,
) -> TcpMessage {
    let order_id = payload.order_id.0.clone();
    log::info!(target: "orders", "Received ProcessPayload for deliver order_id={}", order_id);

    let completed = if let Some(error) = _error {
        TcpMessage::FailedOrder {
            // We don't know the recipe_name here; handle_order will stamp it.
            recipe_name: String::new(),
            error,
        }
    } else {
        payload.updates.push(Update::Deliver {
            timestamp: now_micros(),
        });
        // We don't know the recipe_name here, but handle_order will label it when
        // it receives the CompletedOrder via the channel.
        TcpMessage::CompletedOrder {
            recipe_name: String::new(),
            result: build_result(payload),
        }
    };

    let mut pending_orders = match state.pending_orders.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(tx) = pending_orders.remove(&order_id) {
        let _ = tx.send(completed);
        log::info!(target: "orders", "Delivered from {}", state.identity.addr);
    } else {
        log::info!(target: "orders",
            "Received deliver for unknown order_id={} (no waiting handler)",
            order_id
        );
    }

    // The connection that delivered this message receives a simple ack.
    TcpMessage::Error {
        message: "deliver processed".into(),
    }
}

pub const KNOWN_ACTIONS: &[&str] = &[
    "AddBase",
    "AddBasil",
    "AddBBQSauce",
    "AddBellPepper",
    "AddCheese",
    "AddChicken",
    "AddChiliFlakes",
    "AddGarlic",
    "AddHam",
    "AddMushrooms",
    "AddOliveOil",
    "AddOnion",
    "AddOregano",
    "AddPepperoni",
    "AddPineapple",
    "Bake",
    "MakeDough",
];

/// Map an executed action to the line it appends to the pizza content string.
fn apply_action(action: &crate::protocol::ActionDef, current_content: &str) -> String {
    let line = match action.name.as_str() {
        "MakeDough" => "".to_string(),
        "AddBase" => {
            let base = action
                .params
                .get("base_type")
                .map(String::as_str)
                .unwrap_or("unknown");
            format!("Dough + Base({base}): ready\n")
        }
        "AddCheese" => {
            let amount = action
                .params
                .get("amount")
                .map(String::as_str)
                .unwrap_or("1");
            format!("Cheese x{amount}\n")
        }
        "AddPepperoni" => {
            let slices = action
                .params
                .get("slices")
                .map(String::as_str)
                .unwrap_or("0");
            format!("Pepperoni slices x{slices}\n")
        }
        "Bake" => {
            let duration = action
                .params
                .get("duration")
                .map(String::as_str)
                .unwrap_or("0");
            format!("Baked({duration})\n")
        }
        "AddOliveOil" => "OliveOil: added\n".to_string(),
        "AddMushrooms" => "Mushrooms: added\n".to_string(),
        "AddBasil" => "Basil: added\n".to_string(),
        "AddGarlic" => "Garlic: added\n".to_string(),
        "AddOregano" => "Oregano: added\n".to_string(),
        other => format!("{other}: done\n"),
    };
    format!("{current_content}{line}")
}

/// Serialize the completed payload into the JSON string stored in `CompletedOrder.result`.
/// Consumes the payload so fields can be moved without cloning.
fn build_result(payload: ProcessPayload) -> String {
    let order_result = OrderResult {
        order_id: payload.order_id,
        order_timestamp: payload.order_timestamp,
        content: payload.content,
        updates: payload.updates,
    };
    match serde_json::to_string_pretty(&order_result) {
        Ok(json) => json,
        Err(_) => order_result.content,
    }
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// Send a `Deliver` message to `peer` without reading a response.
///
/// `Deliver` is fire-and-forget: the receiving agent signals its waiting
/// `handle_order` thread and closes the connection immediately, so reading
/// a reply would block indefinitely or return an error.
fn send_deliver(peer: &str, message: &TcpMessage) -> Result<(), String> {
    let mut stream = TcpStream::connect(peer).map_err(|e| format!("connect {peer}: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| format!("set write timeout: {e}"))?;
    let bytes = to_cbor(message).map_err(|e| format!("encode deliver: {e}"))?;
    write_frame(&mut stream, &bytes).map_err(|e| format!("write frame: {e}"))?;
    Ok(())
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
    use std::sync::{Arc, Mutex, RwLock};
    use std::thread;

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
            pending_orders: Mutex::new(HashMap::new()),
        }
    }

    #[test]
    fn handle_list_recipes_returns_local_entries_when_local_recipes_exist() {
        let state = build_state(vec!["MakeDough"], vec!["Margherita"]);
        let response = handle_list_recipes(&state);

        match response {
            TcpMessage::RecipeListAnswer { recipes } => match recipes.get("Margherita") {
                Some(RecipeAvailability::Local { local }) => {
                    assert!(
                        local.missing_actions.is_empty(),
                        "expected local recipe to be complete"
                    );
                }
                other => panic!("expected local availability, got {other:?}"),
            },
            other => panic!("expected RecipeListAnswer, got {other:?}"),
        }
    }

    #[test]
    fn handle_list_recipes_local_missing_actions_account_for_peer_capabilities() {
        let mut state = build_state(vec!["MakeDough"], vec![]);
        state.identity.recipes = vec![Recipe {
            name: "Margherita".to_string(),
            steps: vec![
                Step::Single(ActionDef {
                    name: "MakeDough".to_string(),
                    params: HashMap::new(),
                }),
                Step::Single(ActionDef {
                    name: "AddBase".to_string(),
                    params: HashMap::new(),
                }),
                Step::Single(ActionDef {
                    name: "Bake".to_string(),
                    params: HashMap::new(),
                }),
            ],
            source: "Margherita = MakeDough -> AddBase -> Bake".to_string(),
        }];

        {
            let mut gossip = match state.gossip.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            gossip.peers.insert(
                "127.0.0.1:8002".to_string(),
                PeerInfo {
                    capabilities: vec!["AddBase".to_string()],
                    recipes: vec![],
                    version: Version {
                        counter: 1,
                        generation: 1,
                    },
                    last_seen_us: 1,
                    rtt_us: None,
                },
            );
        }

        let response = handle_list_recipes(&state);
        match response {
            TcpMessage::RecipeListAnswer { recipes } => match recipes.get("Margherita") {
                Some(RecipeAvailability::Local { local }) => {
                    assert_eq!(local.missing_actions, vec!["Bake".to_string()]);
                }
                other => panic!("expected local availability, got {other:?}"),
            },
            other => panic!("expected RecipeListAnswer, got {other:?}"),
        }
    }

    #[test]
    fn handle_list_recipes_returns_remote_host_when_recipe_is_remote_only() {
        let state = build_state(vec![], vec![]);
        {
            let mut gossip = match state.gossip.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            gossip.peers.insert(
                "127.0.0.1:8000".to_string(),
                PeerInfo {
                    capabilities: vec!["MakeDough".to_string()],
                    recipes: vec!["Margherita".to_string()],
                    version: Version {
                        counter: 1,
                        generation: 1,
                    },
                    last_seen_us: 1,
                    rtt_us: None,
                },
            );
        }

        let response = handle_list_recipes(&state);
        match response {
            TcpMessage::RecipeListAnswer { recipes } => match recipes.get("Margherita") {
                Some(RecipeAvailability::Remote { remote }) => {
                    assert_eq!(remote.host.0, "127.0.0.1:8000");
                }
                other => panic!("expected remote availability, got {other:?}"),
            },
            other => panic!("expected RecipeListAnswer, got {other:?}"),
        }
    }

    #[test]
    fn handle_list_recipes_remote_host_is_deterministic_across_multiple_peers() {
        let state = build_state(vec![], vec![]);
        {
            let mut gossip = match state.gossip.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            gossip.peers.insert(
                "127.0.0.1:8002".to_string(),
                PeerInfo {
                    capabilities: vec!["MakeDough".to_string()],
                    recipes: vec!["Margherita".to_string()],
                    version: Version {
                        counter: 1,
                        generation: 1,
                    },
                    last_seen_us: 1,
                    rtt_us: None,
                },
            );
            gossip.peers.insert(
                "127.0.0.1:8000".to_string(),
                PeerInfo {
                    capabilities: vec!["Bake".to_string()],
                    recipes: vec!["Margherita".to_string()],
                    version: Version {
                        counter: 1,
                        generation: 1,
                    },
                    last_seen_us: 1,
                    rtt_us: None,
                },
            );
        }

        let response = handle_list_recipes(&state);
        match response {
            TcpMessage::RecipeListAnswer { recipes } => match recipes.get("Margherita") {
                Some(RecipeAvailability::Remote { remote }) => {
                    assert_eq!(remote.host.0, "127.0.0.1:8000");
                }
                other => panic!("expected remote availability, got {other:?}"),
            },
            other => panic!("expected RecipeListAnswer, got {other:?}"),
        }
    }

    /// Build a shared Arc<NodeState> bound to a random port, also returning the
    /// TcpListener at that port so tests can accept the `deliver` connection.
    fn build_arc_state(
        capabilities: Vec<&str>,
        recipes: Vec<&str>,
    ) -> (Arc<NodeState>, std::net::TcpListener) {
        let deliver_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = deliver_listener.local_addr().unwrap().to_string();

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

        let state = Arc::new(NodeState {
            identity: Identity {
                addr,
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
            pending_orders: Mutex::new(HashMap::new()),
        });

        (state, deliver_listener)
    }

    /// Spawn a thread that accepts exactly one connection on `listener`, decodes the
    /// `Deliver` message, calls `handle_deliver`, and sends an ack back.
    fn spawn_deliver_handler(
        listener: std::net::TcpListener,
        state: Arc<NodeState>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let bytes = read_frame(&mut stream).unwrap();
                let msg: TcpMessage = from_cbor(&bytes).unwrap();
                // No ack written back — deliver is fire-and-forget.
                if let TcpMessage::Deliver { payload, error } = msg {
                    handle_deliver(&state, payload, error);
                }
            }
        })
    }

    #[test]
    fn handle_order_sends_receipt_then_returns_completed_order() {
        // State whose addr == delivery_host — so handle_process_payload delivers to self.
        let (state, deliver_listener) = build_arc_state(vec!["MakeDough"], vec!["Margherita"]);

        // Spin up the mini deliver server before calling handle_order.
        spawn_deliver_handler(deliver_listener, Arc::clone(&state));

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
            matches!(response, TcpMessage::CompletedOrder { ref recipe_name, .. } if recipe_name == "Margherita"),
            "expected CompletedOrder(Margherita) as frame 2, got {response:?}"
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
                    rtt_us: None,
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

        // Bind a listener so handle_process_payload can deliver to it.
        let deliver_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let delivery_addr = deliver_listener.local_addr().unwrap().to_string();

        // Accept the deliver — no ack written back (fire-and-forget protocol).
        let deliver_handle: thread::JoinHandle<TcpMessage> = thread::spawn(move || {
            let (mut stream, _) = deliver_listener.accept().unwrap();
            let bytes = read_frame(&mut stream).unwrap();
            from_cbor(&bytes).unwrap()
        });

        let payload = ProcessPayload {
            order_id: crate::protocol::uuid(uuid::Uuid::nil()),
            order_timestamp: 1,
            delivery_host: crate::protocol::addr(delivery_addr),
            action_index: 0,
            action_sequence: vec![ActionDef {
                name: "MakeDough".to_string(),
                params: HashMap::new(),
            }],
            content: String::new(),
            updates: vec![],
        };

        let _response = handle_process_payload(&state, "TestRecipe", payload);

        // The deliver must have arrived at the listener.
        let received = deliver_handle.join().unwrap();
        match received {
            TcpMessage::Deliver { payload, error } => {
                assert!(error.is_none(), "expected no error in deliver");
                // MakeDough produces no content line; content should be empty.
                assert!(
                    payload.content.is_empty(),
                    "MakeDough should produce no content, got: {:?}",
                    payload.content
                );
                assert_eq!(
                    payload.action_index, 1,
                    "action_index should be past MakeDough"
                );
                // updates: Action{MakeDough} + Forward{delivery_host}
                assert_eq!(
                    payload.updates.len(),
                    2,
                    "expected Action + Forward updates"
                );
            }
            other => panic!("expected Deliver at listener, got {other:?}"),
        }
    }

    #[test]
    fn handle_process_payload_reports_candidate_peer_when_missing_capability() {
        let state = build_state(vec!["Bake"], vec![]);
        let deliver_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let delivery_addr = deliver_listener.local_addr().unwrap().to_string();

        // Accept the failure deliver and inspect its payload + error.
        let deliver_handle: thread::JoinHandle<TcpMessage> = thread::spawn(move || {
            let (mut stream, _) = deliver_listener.accept().unwrap();
            let bytes = read_frame(&mut stream).unwrap();
            from_cbor(&bytes).unwrap()
        });

        let payload = ProcessPayload {
            order_id: crate::protocol::uuid(uuid::Uuid::nil()),
            order_timestamp: 1,
            delivery_host: crate::protocol::addr(delivery_addr),
            action_index: 0,
            action_sequence: vec![ActionDef {
                name: "MakeDough".to_string(),
                params: HashMap::new(),
            }],
            content: String::new(),
            updates: vec![],
        };

        let response = handle_process_payload(&state, "TestRecipe", payload);

        assert!(
            matches!(response, TcpMessage::CompletedOrder { ref result, .. } if result.is_empty()),
            "expected placeholder CompletedOrder after failure deliver, got {response:?}"
        );

        let received = deliver_handle.join().unwrap();
        match received {
            TcpMessage::Deliver { payload, error } => {
                assert_eq!(payload.action_index, 0, "action should not advance");
                assert_eq!(
                    error.as_deref(),
                    Some("Action MakeDough not available"),
                    "expected business failure in deliver"
                );
            }
            other => panic!("expected Deliver, got {other:?}"),
        }
    }

    #[test]
    fn handle_deliver_with_error_signals_failed_order() {
        let state = build_state(vec!["MakeDough"], vec![]);
        let order_id = uuid::Uuid::new_v4();
        let order_id_str = order_id.to_string();
        let (tx, rx) = mpsc::sync_channel(1);
        {
            let mut pending_orders = match state.pending_orders.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            pending_orders.insert(order_id_str.clone(), tx);
        }

        let payload = ProcessPayload {
            order_id: crate::protocol::uuid(order_id),
            order_timestamp: 1,
            delivery_host: crate::protocol::addr("127.0.0.1:9000"),
            action_index: 0,
            action_sequence: vec![],
            content: String::new(),
            updates: vec![],
        };

        let _ack = handle_deliver(
            &state,
            payload,
            Some("Action AddBasil not available".to_string()),
        );
        let delivered = rx.recv().expect("expected failed order through channel");

        assert!(
            matches!(delivered, TcpMessage::FailedOrder { ref error, .. } if error == "Action AddBasil not available"),
            "expected FailedOrder signaled to waiting order handler, got {delivered:?}"
        );
    }

    #[test]
    fn handle_order_returns_failed_order_when_remote_reports_unavailable_action() {
        // Origin agent receives the client order and waits for deliver.
        let (state, deliver_listener) = build_arc_state(vec![], vec![]);
        spawn_deliver_handler(deliver_listener, Arc::clone(&state));

        // Remote peer that advertises recipe + MakeDough but fails on AddBasil.
        let remote_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let remote_addr = remote_listener.local_addr().unwrap().to_string();
        {
            let mut gossip = match state.gossip.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            gossip.peers.insert(
                remote_addr.clone(),
                PeerInfo {
                    capabilities: vec!["MakeDough".to_string()],
                    recipes: vec!["Margherita".to_string()],
                    version: Version {
                        counter: 2,
                        generation: 1,
                    },
                    last_seen_us: 1,
                    rtt_us: None,
                },
            );
        }

        let delivery_addr = state.identity.addr.clone();
        let remote_handle = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = remote_listener.accept().unwrap();
                let req_bytes = read_frame(&mut stream).unwrap();
                let req: TcpMessage = from_cbor(&req_bytes).unwrap();

                match req {
                    TcpMessage::GetRecipe { recipe_name } => {
                        assert_eq!(recipe_name, "Margherita");
                        let recipe = "Margherita = MakeDough -> AddBasil(leaves=3)".to_string();
                        let resp = TcpMessage::RecipeAnswer { recipe };
                        let resp_bytes = to_cbor(&resp).unwrap();
                        write_frame(&mut stream, &resp_bytes).unwrap();
                    }
                    TcpMessage::ProcessPayload { payload } => {
                        // Simulate last-capable peer sending deliver failure to ordering host.
                        let deliver = TcpMessage::Deliver {
                            payload,
                            error: Some("Action AddBasil not available".to_string()),
                        };
                        send_deliver(&delivery_addr, &deliver).unwrap();

                        // Return placeholder response to satisfy forward_to_peer.
                        let resp = TcpMessage::CompletedOrder {
                            recipe_name: "unknown".to_string(),
                            result: String::new(),
                        };
                        let resp_bytes = to_cbor(&resp).unwrap();
                        write_frame(&mut stream, &resp_bytes).unwrap();
                    }
                    other => panic!("unexpected request at remote peer: {other:?}"),
                }
            }
        });

        let (mut client, mut server) = make_stream_pair();
        let response = handle_order(&state, "Margherita", &mut server);

        let receipt_bytes = read_frame(&mut client).unwrap();
        let receipt: TcpMessage = from_cbor(&receipt_bytes).unwrap();
        assert!(
            matches!(receipt, TcpMessage::OrderReceipt { .. }),
            "expected OrderReceipt as frame 1, got {receipt:?}"
        );
        assert!(
            matches!(response, TcpMessage::FailedOrder { ref recipe_name, ref error } if recipe_name == "Margherita" && error == "Action AddBasil not available"),
            "expected FailedOrder as frame 2, got {response:?}"
        );

        remote_handle.join().unwrap();
    }
}
