use ciborium::tag::Required;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::io::Cursor;
use uuid::Uuid;

/// UUID value serialized with native CBOR tag 37.
pub type TaggedUuid = Required<String, 37>;
/// Node address serialized with native CBOR tag 260.
pub type NodeAddr = Required<String, 260>;
/// Last-seen payload can be encoded either with address-string keys
/// or numeric keys depending on peer implementation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum LastSeenMap {
    /// Preferred shape: address -> timestamp.
    ByAddress(HashMap<String, u64>),
    /// Compatibility shape observed from reference binary.
    ByCode(HashMap<i64, u64>),
}
/// Last-seen map serialized with native CBOR tag 1001.
pub type TaggedLastSeen = Required<LastSeenMap, 1001>;

/// Creates a tagged UUID value.
pub fn uuid(value: Uuid) -> TaggedUuid {
    Required(value.to_string())
}

/// Creates a tagged node address value.
pub fn addr(value: impl Into<String>) -> NodeAddr {
    Required(value.into())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Monotonic version tuple used by gossip messages.
pub struct Version {
    /// Logical counter incremented over time.
    pub counter: u64,
    /// Generation timestamp for version ordering.
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// One action in a recipe execution sequence.
pub struct ActionDef {
    /// Action name, e.g. `MakeDough`.
    pub name: String,
    /// Named action parameters.
    #[serde(default)]
    pub params: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
/// Execution trace updates attached to payload forwarding.
pub enum Update {
    /// The payload has been forwarded to another node.
    Forward { to: NodeAddr, timestamp: u64 },
    /// An action has been executed.
    Action { action: ActionDef, timestamp: u64 },
    /// Final delivery marker.
    Deliver { timestamp: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Payload exchanged between peers while executing a recipe.
pub struct ProcessPayload {
    /// Unique order identifier.
    pub order_id: TaggedUuid,
    /// Creation timestamp of the order.
    pub order_timestamp: u64,
    /// Final delivery destination.
    pub delivery_host: NodeAddr,
    /// Current index in `action_sequence`.
    pub action_index: u64,
    /// Planned action sequence for the order.
    pub action_sequence: Vec<ActionDef>,
    /// Payload content being transformed by actions.
    pub content: String,
    /// Forwarding and execution history.
    #[serde(default)]
    pub updates: Vec<Update>,
}

#[derive(Debug, Clone, Serialize)]
/// Subset of `ProcessPayload` returned to the client as a JSON string in `CompletedOrder.result`.
pub struct OrderResult {
    /// Stable order identifier across all hops.
    pub order_id: TaggedUuid,
    /// Creation time of the order in Unix microseconds.
    pub order_timestamp: u64,
    /// Human-readable pizza description accumulated by actions.
    pub content: String,
    /// Append-only execution trace (Forward / Action / Deliver).
    pub updates: Vec<Update>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Gossip announcement broadcast by a node.
pub struct Announce {
    /// Announcing node address.
    pub node_addr: NodeAddr,
    /// Capabilities advertised by the node.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Recipe names advertised by the node.
    #[serde(default)]
    pub recipes: Vec<String>,
    /// Neighbors known by the announcing node.
    #[serde(default)]
    pub peers: Vec<NodeAddr>,
    /// Version associated with this announcement.
    pub version: Version,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Gossip liveness check payload.
pub struct Check {
    /// Last-seen timestamps by peer address.
    pub last_seen: TaggedLastSeen,
    /// Version observed by sender.
    pub version: Version,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
/// UDP gossip message types.
pub enum UdpMessage {
    Announce(Announce),
    Ping(Check),
    Pong(Check),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Local recipe execution status on the responding node.
pub struct RecipeStatus {
    /// Actions required but not executable locally.
    #[serde(default)]
    pub missing_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Remote recipe availability pointing to one host.
pub struct RemoteRecipeStatus {
    pub host: NodeAddr,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(untagged)]
/// Canonical recipe availability shape: either local or remote.
pub enum RecipeAvailability {
    Local { local: RecipeStatus },
    Remote { remote: RemoteRecipeStatus },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RecipeAvailabilityWire {
    Local {
        local: RecipeStatus,
    },
    Remote {
        remote: RemoteRecipeStatus,
    },
    LegacyFull {
        local: RecipeStatus,
        #[serde(default)]
        remote_peers: Vec<String>,
    },
    LegacyFlat {
        #[serde(default)]
        missing_actions: Vec<String>,
        #[serde(default)]
        remote_peers: Vec<String>,
    },
    LegacyRemoteOnly {
        #[serde(default)]
        remote_peers: Vec<String>,
    },
}

fn pick_remote_host(mut remote_peers: Vec<String>) -> Option<String> {
    if remote_peers.is_empty() {
        None
    } else {
        remote_peers.sort();
        remote_peers.into_iter().next()
    }
}

impl<'de> Deserialize<'de> for RecipeAvailability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = RecipeAvailabilityWire::deserialize(deserializer)?;
        Ok(match wire {
            RecipeAvailabilityWire::Local { local } => Self::Local { local },
            RecipeAvailabilityWire::Remote { remote } => Self::Remote { remote },
            RecipeAvailabilityWire::LegacyFull {
                local,
                remote_peers,
            } => {
                let _ = remote_peers;
                Self::Local { local }
            }
            RecipeAvailabilityWire::LegacyFlat {
                missing_actions,
                remote_peers,
            } => {
                if missing_actions.is_empty() {
                    if let Some(host) = pick_remote_host(remote_peers) {
                        Self::Remote {
                            remote: RemoteRecipeStatus { host: addr(host) },
                        }
                    } else {
                        Self::Local {
                            local: RecipeStatus { missing_actions },
                        }
                    }
                } else {
                    Self::Local {
                        local: RecipeStatus { missing_actions },
                    }
                }
            }
            RecipeAvailabilityWire::LegacyRemoteOnly { remote_peers } => {
                if let Some(host) = pick_remote_host(remote_peers) {
                    Self::Remote {
                        remote: RemoteRecipeStatus { host: addr(host) },
                    }
                } else {
                    Self::Local {
                        local: RecipeStatus {
                            missing_actions: vec![],
                        },
                    }
                }
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// TCP request/response messages exchanged between client and nodes.
pub enum TcpMessage {
    ListRecipes,
    ListCapabilities,
    CapabilitiesAnswer {
        capabilities: Vec<String>,
    },
    RecipeListAnswer {
        recipes: HashMap<String, RecipeAvailability>,
    },
    Order {
        recipe_name: String,
    },
    OrderReceipt {
        order_id: TaggedUuid,
    },
    GetRecipe {
        recipe_name: String,
    },
    RecipeAnswer {
        recipe: String,
    },
    ProcessPayload {
        payload: ProcessPayload,
    },
    CompletedOrder {
        recipe_name: String,
        result: String,
    },
    FailedOrder {
        recipe_name: String,
        error: String,
    },
    OrderDeclined {
        message: String,
    },
    Deliver {
        payload: ProcessPayload,
        error: Option<String>,
    },
    Error {
        message: String,
    },
}

/// Serialize a value into CBOR bytes.
pub fn to_cbor<T: Serialize>(value: &T) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(value, &mut out)?;
    Ok(out)
}

/// Deserialize a value from CBOR bytes.
pub fn from_cbor<T: DeserializeOwned>(
    bytes: &[u8],
) -> Result<T, ciborium::de::Error<std::io::Error>> {
    let cursor = Cursor::new(bytes);
    ciborium::de::from_reader(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "snake_case")]
    enum LegacyTcpMessage {
        RecipeListAnswer {
            recipes: HashMap<String, LegacyRecipeAvailability>,
        },
    }

    #[derive(Debug, Clone, Serialize)]
    struct LegacyRecipeAvailability {
        #[serde(default)]
        missing_actions: Vec<String>,
        #[serde(default)]
        remote_peers: Vec<String>,
    }

    #[test]
    fn udp_message_roundtrip() {
        let msg = UdpMessage::Announce(Announce {
            node_addr: addr("127.0.0.1:8000"),
            capabilities: vec!["MakeDough".to_string()],
            recipes: vec!["Pepperoni".to_string()],
            peers: vec![addr("127.0.0.1:8002")],
            version: Version {
                counter: 3,
                generation: 1_773_591_739,
            },
        });

        let encoded = to_cbor(&msg).unwrap();
        let decoded: UdpMessage = from_cbor(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn tcp_process_payload_roundtrip() {
        let msg = TcpMessage::ProcessPayload {
            payload: ProcessPayload {
                order_id: uuid(Uuid::nil()),
                order_timestamp: 1_773_599_028_742_680,
                delivery_host: addr("127.0.0.1:8002"),
                action_index: 0,
                action_sequence: vec![ActionDef {
                    name: "MakeDough".to_string(),
                    params: HashMap::new(),
                }],
                content: String::new(),
                updates: vec![Update::Forward {
                    to: addr("127.0.0.1:8000"),
                    timestamp: 1_773_599_028_758_515,
                }],
            },
        };

        let encoded = to_cbor(&msg).unwrap();
        let decoded: TcpMessage = from_cbor(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn tcp_failed_order_roundtrip() {
        let msg = TcpMessage::FailedOrder {
            recipe_name: "Margherita".to_string(),
            error: "Action AddBasil not available".to_string(),
        };

        let encoded = to_cbor(&msg).unwrap();
        let decoded: TcpMessage = from_cbor(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn list_recipes_local_serializes_with_local_key() {
        let mut recipes = HashMap::new();
        recipes.insert(
            "Pepperoni".to_string(),
            RecipeAvailability::Local {
                local: RecipeStatus {
                    missing_actions: vec!["Bake".to_string()],
                },
            },
        );
        let msg = TcpMessage::RecipeListAnswer { recipes };
        let encoded = to_cbor(&msg).unwrap();

        assert!(
            encoded.windows("local".len()).any(|w| w == b"local"),
            "expected serialized payload to contain 'local' key"
        );
        assert!(
            !encoded.windows("remote".len()).any(|w| w == b"remote"),
            "local-only response should not contain 'remote' key"
        );
    }

    #[test]
    fn list_recipes_remote_serializes_with_remote_host_and_tag_260() {
        let mut recipes = HashMap::new();
        recipes.insert(
            "Margherita".to_string(),
            RecipeAvailability::Remote {
                remote: RemoteRecipeStatus {
                    host: addr("127.0.0.1:8000"),
                },
            },
        );
        let msg = TcpMessage::RecipeListAnswer { recipes };
        let encoded = to_cbor(&msg).unwrap();

        assert!(
            encoded.windows("remote".len()).any(|w| w == b"remote"),
            "expected serialized payload to contain 'remote' key"
        );
        assert!(
            encoded.windows("host".len()).any(|w| w == b"host"),
            "expected serialized payload to contain 'host' key"
        );
        assert!(
            encoded.windows(3).any(|w| w == [0xD9, 0x01, 0x04]),
            "expected CBOR tag 260 marker for host"
        );
    }

    #[test]
    fn list_recipes_legacy_remote_peers_decodes_to_remote_host() {
        let mut recipes = HashMap::new();
        recipes.insert(
            "Funghi".to_string(),
            LegacyRecipeAvailability {
                missing_actions: vec![],
                remote_peers: vec!["127.0.0.1:8002".to_string(), "127.0.0.1:8000".to_string()],
            },
        );
        let legacy = LegacyTcpMessage::RecipeListAnswer { recipes };
        let encoded = to_cbor(&legacy).unwrap();
        let decoded: TcpMessage = from_cbor(&encoded).unwrap();

        match decoded {
            TcpMessage::RecipeListAnswer { recipes } => match recipes.get("Funghi") {
                Some(RecipeAvailability::Remote { remote }) => {
                    assert_eq!(remote.host.0, "127.0.0.1:8000");
                }
                other => panic!("expected remote availability, got {other:?}"),
            },
            other => panic!("expected RecipeListAnswer, got {other:?}"),
        }
    }
}
