use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::io::Cursor;
use uuid::Uuid;

/// CBOR tag used for UUID values.
pub const UUID: u64 = 37;
/// CBOR tag used for node address values.
pub const ADDR: u64 = 260;
/// CBOR tag used for last-seen maps.
pub const LAST_SEEN: u64 = 1001;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Monotonic version tuple used by gossip messages.
pub struct Version {
    /// Logical counter incremented over time.
    pub counter: u64,
    /// Generation timestamp for version ordering.
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Generic tagged value used for typed CBOR payloads.
pub struct Tagged<T> {
    /// Numeric tag identifier.
    pub tag: u64,
    /// Wrapped payload value.
    pub value: T,
}

impl<T> Tagged<T> {
    /// Creates a tagged payload.
    pub fn new(tag: u64, value: T) -> Self {
        Self { tag, value }
    }
}

/// UUID value serialized with a dedicated CBOR tag.
pub type TaggedUuid = Tagged<Uuid>;
/// Node address serialized with a dedicated CBOR tag.
pub type NodeAddr = Tagged<String>;
/// Last-seen map serialized with a dedicated CBOR tag.
pub type TaggedLastSeen = Tagged<HashMap<String, u64>>;

impl Tagged<Uuid> {
    /// Builds a tagged UUID value.
    pub fn uuid(value: Uuid) -> Self {
        Self::new(UUID, value)
    }
}

impl Tagged<String> {
    /// Builds a tagged node address value.
    pub fn addr(value: impl Into<String>) -> Self {
        Self::new(ADDR, value.into())
    }
}

impl Tagged<HashMap<String, u64>> {
    /// Builds a tagged last-seen map value.
    pub fn last_seen(value: HashMap<String, u64>) -> Self {
        Self::new(LAST_SEEN, value)
    }
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
/// Aggregated recipe availability (local and discovered remote peers).
pub struct RecipeAvailability {
    /// Local status for this recipe on the current node.
    pub local: RecipeStatus,
    /// Remote peers known (via gossip) to advertise this recipe.
    #[serde(default)]
    pub remote_peers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// TCP request/response messages exchanged between client and nodes.
pub enum TcpMessage {
    ListRecipes,
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

    #[test]
    fn udp_message_roundtrip() {
        let msg = UdpMessage::Announce(Announce {
            node_addr: Tagged::addr("127.0.0.1:8000"),
            capabilities: vec!["MakeDough".to_string()],
            recipes: vec!["Pepperoni".to_string()],
            peers: vec![Tagged::addr("127.0.0.1:8002")],
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
                order_id: Tagged::uuid(Uuid::nil()),
                order_timestamp: 1_773_599_028_742_680,
                delivery_host: Tagged::addr("127.0.0.1:8002"),
                action_index: 0,
                action_sequence: vec![ActionDef {
                    name: "MakeDough".to_string(),
                    params: HashMap::new(),
                }],
                content: String::new(),
                updates: vec![Update::Forward {
                    to: Tagged::addr("127.0.0.1:8000"),
                    timestamp: 1_773_599_028_758_515,
                }],
            },
        };

        let encoded = to_cbor(&msg).unwrap();
        let decoded: TcpMessage = from_cbor(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn check_roundtrip() {
        let mut last_seen_map = HashMap::new();
        last_seen_map.insert("127.0.0.1:8002".to_string(), 1_773_591_739);
        let check: Check = Check {
            last_seen: Tagged::last_seen(last_seen_map),
            version: Version {
                counter: 1,
                generation: 12345,
            },
        };

        let encoded = to_cbor(&check).unwrap();
        let decoded: Check = from_cbor(&encoded).unwrap();
        assert_eq!(decoded, check);
    }
}
