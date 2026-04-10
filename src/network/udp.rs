use std::collections::HashMap;
use std::io::Result;
use std::net::{SocketAddr, UdpSocket};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::{from_cbor, to_cbor, Announce, Tagged, UdpMessage, Version};

#[derive(Debug, Clone)]
pub struct GossipState {
    pub self_addr: String,
    pub capabilities: Vec<String>,
    pub recipes: Vec<String>,
    pub peers: HashMap<String, u64>,
    pub version: Version,
}

impl GossipState {
    pub fn new(self_addr: String, capabilities: Vec<String>, recipes: Vec<String>) -> Self {
        Self {
            self_addr,
            capabilities,
            recipes,
            peers: HashMap::new(),
            version: Version {
                counter: 1,
                generation: now_secs(),
            },
        }
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn send_datagram(socket: &UdpSocket, payload: &[u8], target: &str) -> Result<()> {
    socket.send_to(payload, target)?;
    Ok(())
}

pub fn recv_datagram(socket: &UdpSocket) -> Result<(Vec<u8>, SocketAddr)> {
    let mut buf = vec![0u8; 65535];
    let (len, addr) = socket.recv_from(&mut buf)?;
    buf.truncate(len);
    Ok((buf, addr))
}

pub fn encode_udp_message(message: &UdpMessage) -> Result<Vec<u8>> {
    to_cbor(message).map_err(std::io::Error::other)
}

pub fn decode_udp_message(bytes: &[u8]) -> Result<UdpMessage> {
    from_cbor(bytes).map_err(std::io::Error::other)
}

pub fn send_udp_message(socket: &UdpSocket, message: &UdpMessage, target: &str) -> Result<()> {
    let payload = encode_udp_message(message)?;
    send_datagram(socket, &payload, target)
}

pub fn build_announce(state: &GossipState, peers: Vec<String>) -> UdpMessage {
    UdpMessage::Announce(Announce {
        node_addr: Tagged::addr(state.self_addr.clone()),
        capabilities: state.capabilities.clone(),
        recipes: state.recipes.clone(),
        peers: peers.into_iter().map(Tagged::addr).collect(),
        version: state.version.clone(),
    })
}

pub fn apply_announce(state: &mut GossipState, announce: &Announce) {
    state
        .peers
        .insert(announce.node_addr.value.clone(), now_secs());

    if is_newer_version(&announce.version, &state.version) {
        state.version = announce.version.clone();
    }
}

pub fn is_newer_version(candidate: &Version, current: &Version) -> bool {
    (candidate.generation, candidate.counter) > (current.generation, current.counter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Check, Tagged, Version};
    use std::collections::HashMap;

    #[test]
    fn test_send_recv_datagram() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = receiver.local_addr().unwrap();

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        send_datagram(&sender, &b"announce pizza"[..], &addr.to_string()).unwrap();

        let (data, _) = recv_datagram(&receiver).unwrap();
        assert_eq!(data, b"announce pizza");
    }

    #[test]
    fn test_adresse_expediteur() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let recv_addr = receiver.local_addr().unwrap();

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_addr = sender.local_addr().unwrap();

        send_datagram(&sender, &b"ping"[..], &recv_addr.to_string()).unwrap();

        let (data, from) = recv_datagram(&receiver).unwrap();
        assert_eq!(data, b"ping");
        assert_eq!(from, sender_addr);
    }

    #[test]
    fn udp_message_roundtrip_over_helpers() {
        let message = UdpMessage::Announce(Announce {
            node_addr: Tagged::addr("127.0.0.1:8000"),
            capabilities: vec!["MakeDough".to_string()],
            recipes: vec!["Margherita".to_string()],
            peers: vec![Tagged::addr("127.0.0.1:8002")],
            version: Version {
                counter: 3,
                generation: 1_773_591_739,
            },
        });

        let encoded = encode_udp_message(&message).unwrap();
        let decoded = decode_udp_message(&encoded).unwrap();

        assert_eq!(decoded, message);
    }

    #[test]
    fn build_announce_contains_local_state() {
        let state = GossipState::new(
            "127.0.0.1:8000".to_string(),
            vec!["MakeDough".to_string()],
            vec!["Margherita".to_string()],
        );

        let message = build_announce(&state, vec!["127.0.0.1:8002".to_string()]);

        match message {
            UdpMessage::Announce(announce) => {
                assert_eq!(announce.node_addr, Tagged::addr("127.0.0.1:8000"));
                assert_eq!(announce.capabilities, vec!["MakeDough".to_string()]);
                assert_eq!(announce.recipes, vec!["Margherita".to_string()]);
                assert_eq!(announce.peers, vec![Tagged::addr("127.0.0.1:8002")]);
                assert_eq!(announce.version.counter, 1);
            }
            _ => panic!("expected Announce message"),
        }
    }

    #[test]
    fn apply_announce_updates_peer_and_version() {
        let mut state = GossipState::new(
            "127.0.0.1:8000".to_string(),
            vec!["MakeDough".to_string()],
            vec![],
        );

        let announce = Announce {
            node_addr: Tagged::addr("127.0.0.1:8002"),
            capabilities: vec!["AddCheese".to_string()],
            recipes: vec![],
            peers: vec![Tagged::addr("127.0.0.1:8000")],
            version: Version {
                counter: 3,
                generation: state.version.generation + 1,
            },
        };

        apply_announce(&mut state, &announce);

        assert!(state.peers.contains_key("127.0.0.1:8002"));
        assert_eq!(state.version, announce.version);
    }

    #[test]
    fn udp_message_send_and_receive() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let receiver_addr = receiver.local_addr().unwrap();

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();

        let mut last_seen = HashMap::new();
        last_seen.insert(receiver_addr.to_string(), 123);

        let message = UdpMessage::Ping(Check {
            last_seen: Tagged::last_seen(last_seen),
            version: Version {
                counter: 1,
                generation: 42,
            },
        });

        send_udp_message(&sender, &message, &receiver_addr.to_string()).unwrap();

        let (bytes, from) = recv_datagram(&receiver).unwrap();
        let decoded = decode_udp_message(&bytes).unwrap();

        assert_eq!(from, sender.local_addr().unwrap());
        assert_eq!(decoded, message);
    }
}
