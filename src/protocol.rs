use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::io::Cursor;
use uuid::Uuid;

pub const UUID: u64 = 37;
pub const ADDR: u64 = 260;
pub const LAST_SEEN: u64 = 1001;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Version {
    pub counter: u64,
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tagged<T> {
    pub tag: u64,
    pub value: T,
}

impl<T> Tagged<T> {
    pub fn new(tag: u64, value: T) -> Self {
        Self { tag, value }
    }
}

pub type TaggedUuid = Tagged<Uuid>;
pub type NodeAddr = Tagged<String>;
pub type TaggedLastSeen = Tagged<HashMap<String, u64>>;

impl Tagged<Uuid> {
    pub fn uuid(value: Uuid) -> Self {
        Self::new(UUID, value)
    }
}

impl Tagged<String> {
    pub fn addr(value: impl Into<String>) -> Self {
        Self::new(ADDR, value.into())
    }
}

impl Tagged<HashMap<String, u64>> {
    pub fn last_seen(value: HashMap<String, u64>) -> Self {
        Self::new(LAST_SEEN, value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionDef {
    pub name: String,
    #[serde(default)]
    pub params: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum Update {
    Forward { to: NodeAddr, timestamp: u64 },
    Action { action: ActionDef, timestamp: u64 },
    Deliver { timestamp: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessPayload {
    pub order_id: TaggedUuid,
    pub order_timestamp: u64,
    pub delivery_host: NodeAddr,
    pub action_index: u64,
    pub action_sequence: Vec<ActionDef>,
    pub content: String,
    #[serde(default)]
    pub updates: Vec<Update>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Announce {
    pub node_addr: NodeAddr,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub recipes: Vec<String>,
    #[serde(default)]
    pub peers: Vec<NodeAddr>,
    pub version: Version,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Check {
    pub last_seen: TaggedLastSeen,
    pub version: Version,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum UdpMessage {
    Announce(Announce),
    Ping(Check),
    Pong(Check),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecipeStatus {
    #[serde(default)]
    pub missing_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecipeAvailability {
    pub local: RecipeStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
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

pub fn to_cbor<T: Serialize>(value: &T) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(value, &mut out)?;
    Ok(out)
}

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
