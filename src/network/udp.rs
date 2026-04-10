use std::collections::HashMap;
use std::io::Result;
use std::net::{SocketAddr, UdpSocket};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::{Announce, Check, Tagged, UdpMessage, Version, from_cbor, to_cbor};

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

pub fn mark_peer_seen(state: &mut GossipState, peer_addr: &str) {
    state.peers.insert(peer_addr.to_string(), now_secs());
}

pub fn build_ping(state: &GossipState) -> UdpMessage {
    let mut last_seen = HashMap::new();
    last_seen.insert(state.self_addr.clone(), now_secs());

    UdpMessage::Ping(Check {
        last_seen: Tagged::last_seen(last_seen),
        version: state.version.clone(),
    })
}

pub fn build_pong(state: &GossipState) -> UdpMessage {
    let mut last_seen = HashMap::new();
    last_seen.insert(state.self_addr.clone(), now_secs());

    UdpMessage::Pong(Check {
        last_seen: Tagged::last_seen(last_seen),
        version: state.version.clone(),
    })
}

pub fn apply_check(state: &mut GossipState, peer_addr: &str, check: &Check) {
    mark_peer_seen(state, peer_addr);

    if is_newer_version(&check.version, &state.version) {
        state.version = check.version.clone();
    }
}

pub fn handle_udp_message(
    state: &mut GossipState,
    peer_addr: &str,
    message: &UdpMessage,
) -> Option<UdpMessage> {
    match message {
        UdpMessage::Announce(announce) => {
            apply_announce(state, announce);
            None
        }
        UdpMessage::Ping(check) => {
            apply_check(state, peer_addr, check);
            Some(build_pong(state))
        }
        UdpMessage::Pong(check) => {
            apply_check(state, peer_addr, check);
            None
        }
    }
}

pub fn process_one_datagram(socket: &UdpSocket, state: &mut GossipState) -> Result<UdpMessage> {
    let (bytes, from) = recv_datagram(socket)?;
    let message = decode_udp_message(&bytes)?;

    if let Some(reply) = handle_udp_message(state, &from.to_string(), &message) {
        send_udp_message(socket, &reply, &from.to_string())?;
    }

    Ok(message)
}

pub fn send_initial_announces(
    socket: &UdpSocket,
    state: &GossipState,
    peers: &[String],
) -> Result<usize> {
    let announce = build_announce(state, peers.to_vec());
    let mut sent = 0usize;

    for peer in peers {
        if peer == &state.self_addr {
            continue;
        }
        send_udp_message(socket, &announce, peer)?;
        sent += 1;
    }

    Ok(sent)
}

pub fn run_gossip_steps(
    socket: &UdpSocket,
    state: &mut GossipState,
    peers: &[String],
    steps: usize,
) -> Result<()> {
    send_initial_announces(socket, state, peers)?;

    for _ in 0..steps {
        process_one_datagram(socket, state)?;
    }

    Ok(())
}

pub fn run_gossip_service(
    socket: &UdpSocket,
    state: &mut GossipState,
    peers: &[String],
) -> Result<()> {
    send_initial_announces(socket, state, peers)?;

    loop {
        process_one_datagram(socket, state)?;
    }
}

pub fn send_ping_to_known_peers(socket: &UdpSocket, state: &GossipState) -> Result<usize> {
    let ping = build_ping(state);
    let mut sent = 0usize;

    for peer_addr in state.peers.keys() {
        if peer_addr == &state.self_addr {
            continue;
        }
        send_udp_message(socket, &ping, peer_addr)?;
        sent += 1;
    }

    Ok(sent)
}

fn message_version_counter(message: &UdpMessage) -> u64 {
    match message {
        UdpMessage::Announce(announce) => announce.version.counter,
        UdpMessage::Ping(check) | UdpMessage::Pong(check) => check.version.counter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Tagged, Version};
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
    fn mark_peer_seen_updates_presence_map() {
        let mut state = GossipState::new(
            "127.0.0.1:8000".to_string(),
            vec!["MakeDough".to_string()],
            vec![],
        );

        mark_peer_seen(&mut state, "127.0.0.1:8002");

        assert!(state.peers.contains_key("127.0.0.1:8002"));
    }

    #[test]
    fn build_ping_contains_local_version_and_last_seen() {
        let state = GossipState::new(
            "127.0.0.1:8000".to_string(),
            vec!["MakeDough".to_string()],
            vec![],
        );

        let message = build_ping(&state);

        match message {
            UdpMessage::Ping(check) => {
                assert_eq!(check.version, state.version);
                assert!(check.last_seen.value.contains_key("127.0.0.1:8000"));
            }
            _ => panic!("expected Ping message"),
        }
    }

    #[test]
    fn build_pong_contains_local_version_and_last_seen() {
        let state = GossipState::new(
            "127.0.0.1:8000".to_string(),
            vec!["MakeDough".to_string()],
            vec![],
        );

        let message = build_pong(&state);

        match message {
            UdpMessage::Pong(check) => {
                assert_eq!(check.version, state.version);
                assert!(check.last_seen.value.contains_key("127.0.0.1:8000"));
            }
            _ => panic!("expected Pong message"),
        }
    }

    #[test]
    fn apply_check_updates_version_when_newer() {
        let mut state = GossipState::new(
            "127.0.0.1:8000".to_string(),
            vec!["MakeDough".to_string()],
            vec![],
        );

        let check = Check {
            last_seen: Tagged::last_seen(HashMap::new()),
            version: Version {
                counter: state.version.counter + 1,
                generation: state.version.generation + 1,
            },
        };

        apply_check(&mut state, "127.0.0.1:8002", &check);

        assert!(state.peers.contains_key("127.0.0.1:8002"));
        assert_eq!(state.version, check.version);
    }

    #[test]
    fn apply_check_keeps_version_when_older() {
        let mut state = GossipState::new(
            "127.0.0.1:8000".to_string(),
            vec!["MakeDough".to_string()],
            vec![],
        );
        let original_version = state.version.clone();

        let check = Check {
            last_seen: Tagged::last_seen(HashMap::new()),
            version: Version {
                counter: 0,
                generation: original_version.generation.saturating_sub(1),
            },
        };

        apply_check(&mut state, "127.0.0.1:8002", &check);

        assert!(state.peers.contains_key("127.0.0.1:8002"));
        assert_eq!(state.version, original_version);
    }

    #[test]
    fn handle_udp_message_updates_state_on_announce() {
        let mut state = GossipState::new(
            "127.0.0.1:8000".to_string(),
            vec!["MakeDough".to_string()],
            vec![],
        );

        let message = UdpMessage::Announce(Announce {
            node_addr: Tagged::addr("127.0.0.1:8002"),
            capabilities: vec!["AddCheese".to_string()],
            recipes: vec![],
            peers: vec![Tagged::addr("127.0.0.1:8000")],
            version: Version {
                counter: state.version.counter + 1,
                generation: state.version.generation + 1,
            },
        });

        let response = handle_udp_message(&mut state, "127.0.0.1:8002", &message);

        assert!(response.is_none());
        assert!(state.peers.contains_key("127.0.0.1:8002"));
        assert_eq!(state.version.counter, message_version_counter(&message));
    }

    #[test]
    fn handle_udp_message_responds_to_ping_with_pong() {
        let mut state = GossipState::new(
            "127.0.0.1:8000".to_string(),
            vec!["MakeDough".to_string()],
            vec![],
        );

        let message = UdpMessage::Ping(Check {
            last_seen: Tagged::last_seen(HashMap::new()),
            version: Version {
                counter: state.version.counter + 1,
                generation: state.version.generation + 1,
            },
        });

        let response = handle_udp_message(&mut state, "127.0.0.1:8002", &message);

        assert!(state.peers.contains_key("127.0.0.1:8002"));
        match response {
            Some(UdpMessage::Pong(check)) => {
                assert_eq!(check.version, state.version);
                assert!(check.last_seen.value.contains_key("127.0.0.1:8000"));
            }
            _ => panic!("expected Pong response"),
        }
    }

    #[test]
    fn process_one_datagram_receives_ping_and_sends_pong() {
        let service_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let service_addr = service_socket.local_addr().unwrap();

        let peer_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        peer_socket
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();

        let mut state = GossipState::new(
            service_addr.to_string(),
            vec!["MakeDough".to_string()],
            vec![],
        );

        let ping = UdpMessage::Ping(Check {
            last_seen: Tagged::last_seen(HashMap::new()),
            version: Version {
                counter: state.version.counter + 1,
                generation: state.version.generation + 1,
            },
        });

        send_udp_message(&peer_socket, &ping, &service_addr.to_string()).unwrap();

        let received = process_one_datagram(&service_socket, &mut state).unwrap();
        assert_eq!(received, ping);

        let (reply_bytes, _) = recv_datagram(&peer_socket).unwrap();
        let reply = decode_udp_message(&reply_bytes).unwrap();
        assert!(matches!(reply, UdpMessage::Pong(_)));
    }

    #[test]
    fn send_ping_to_known_peers_sends_ping_to_all_neighbors() {
        let receiver_a = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver_a
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let addr_a = receiver_a.local_addr().unwrap();

        let receiver_b = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver_b
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let addr_b = receiver_b.local_addr().unwrap();

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_addr = sender.local_addr().unwrap();

        let mut state = GossipState::new(
            sender_addr.to_string(),
            vec!["MakeDough".to_string()],
            vec![],
        );
        state.peers.insert(addr_a.to_string(), now_secs());
        state.peers.insert(addr_b.to_string(), now_secs());

        let sent = send_ping_to_known_peers(&sender, &state).unwrap();
        assert_eq!(sent, 2);

        let (bytes_a, from_a) = recv_datagram(&receiver_a).unwrap();
        let (bytes_b, from_b) = recv_datagram(&receiver_b).unwrap();

        assert_eq!(from_a, sender_addr);
        assert_eq!(from_b, sender_addr);

        assert!(matches!(decode_udp_message(&bytes_a).unwrap(), UdpMessage::Ping(_)));
        assert!(matches!(decode_udp_message(&bytes_b).unwrap(), UdpMessage::Ping(_)));
    }

    #[test]
    fn send_initial_announces_sends_announce_to_peers() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let receiver_addr = receiver.local_addr().unwrap();

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_addr = sender.local_addr().unwrap();

        let state = GossipState::new(
            sender_addr.to_string(),
            vec!["MakeDough".to_string()],
            vec!["Margherita".to_string()],
        );

        let peers = vec![receiver_addr.to_string()];
        let sent = send_initial_announces(&sender, &state, &peers).unwrap();
        assert_eq!(sent, 1);

        let (bytes, from) = recv_datagram(&receiver).unwrap();
        let message = decode_udp_message(&bytes).unwrap();

        assert_eq!(from, sender_addr);
        match message {
            UdpMessage::Announce(announce) => {
                assert_eq!(announce.node_addr, Tagged::addr(sender_addr.to_string()));
                assert!(announce.capabilities.contains(&"MakeDough".to_string()));
            }
            _ => panic!("expected Announce message"),
        }
    }

    #[test]
    fn run_gossip_steps_processes_one_ping_and_replies_with_pong() {
        let service_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let service_addr = service_socket.local_addr().unwrap();

        let peer_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        peer_socket
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();

        let mut state = GossipState::new(
            service_addr.to_string(),
            vec!["MakeDough".to_string()],
            vec![],
        );

        let ping = UdpMessage::Ping(Check {
            last_seen: Tagged::last_seen(HashMap::new()),
            version: Version {
                counter: state.version.counter + 1,
                generation: state.version.generation + 1,
            },
        });

        send_udp_message(&peer_socket, &ping, &service_addr.to_string()).unwrap();

        let no_peers: Vec<String> = Vec::new();
        run_gossip_steps(&service_socket, &mut state, &no_peers, 1).unwrap();

        let (reply_bytes, _) = recv_datagram(&peer_socket).unwrap();
        let reply = decode_udp_message(&reply_bytes).unwrap();
        assert!(matches!(reply, UdpMessage::Pong(_)));
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
