use std::collections::HashMap;
use std::io::Result;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::node::{NodeState, PeerInfo};
use crate::protocol::{Announce, Check, Tagged, UdpMessage, Version, from_cbor, to_cbor};

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
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

pub fn is_newer_version(candidate: &Version, current: &Version) -> bool {
    (candidate.generation, candidate.counter) > (current.generation, current.counter)
}

pub fn run_gossip_service_shared(
    socket: &UdpSocket,
    node_state: Arc<NodeState>,
    peers: &[String],
) -> Result<()> {
    let announce = build_announce_from_node(&node_state, peers.to_vec());
    for peer in peers {
        if peer == &node_state.identity.addr {
            continue;
        }
        send_udp_message(socket, &announce, peer)?;
    }

    loop {
        gossip_tick_shared(socket, &node_state)?;
    }
}

pub fn gossip_tick_shared(socket: &UdpSocket, node_state: &Arc<NodeState>) -> Result<UdpMessage> {
    // Send Announces to all known peers to propagate recipe/capability updates
    {
        let gossip = node_state.gossip.read().unwrap();
        let peer_addrs: Vec<String> = gossip.peers.keys().cloned().collect();
        drop(gossip);

        let announce = build_announce_from_node(node_state, peer_addrs.clone());
        for peer_addr in peer_addrs {
            if peer_addr != node_state.identity.addr {
                let _ = send_udp_message(socket, &announce, &peer_addr);
            }
        }
    }

    let _ = send_ping_to_known_peers_shared(socket, node_state)?;
    process_one_datagram_shared(socket, node_state)
}

pub fn send_ping_to_known_peers_shared(
    socket: &UdpSocket,
    node_state: &Arc<NodeState>,
) -> Result<usize> {
    let ping = build_ping_from_node(node_state);
    let peer_addrs: Vec<String> = {
        let gossip = node_state.gossip.read().unwrap();
        gossip.peers.keys().cloned().collect()
    };

    let mut sent = 0usize;
    for peer_addr in peer_addrs {
        if peer_addr == node_state.identity.addr {
            continue;
        }
        send_udp_message(socket, &ping, &peer_addr)?;
        sent += 1;
    }

    Ok(sent)
}

pub fn process_one_datagram_shared(
    socket: &UdpSocket,
    node_state: &Arc<NodeState>,
) -> Result<UdpMessage> {
    match recv_datagram(socket) {
        Ok((bytes, from)) => {
            let message = decode_udp_message(&bytes)?;
            let from_addr = from.to_string();

            if let Some(reply) = handle_udp_message_shared(node_state, &from_addr, &message) {
                send_udp_message(socket, &reply, &from_addr)?;
            }

            Ok(message)
        }
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            // Timeout or would-block: return empty Pong so the loop continues
            Ok(UdpMessage::Pong(Check {
                last_seen: Tagged::last_seen(HashMap::new()),
                version: Version {
                    counter: 0,
                    generation: 0,
                },
            }))
        }
        Err(e) => Err(e),
    }
}

pub fn handle_udp_message_shared(
    node_state: &Arc<NodeState>,
    peer_addr: &str,
    message: &UdpMessage,
) -> Option<UdpMessage> {
    match message {
        UdpMessage::Announce(announce) => {
            apply_announce_shared(node_state, announce);
            None
        }
        UdpMessage::Ping(check) => {
            apply_check_shared(node_state, peer_addr, check);
            Some(build_pong_from_node(node_state))
        }
        UdpMessage::Pong(check) => {
            apply_check_shared(node_state, peer_addr, check);
            None
        }
    }
}

fn build_announce_from_node(node_state: &Arc<NodeState>, peers: Vec<String>) -> UdpMessage {
    let version = {
        let gossip = node_state.gossip.read().unwrap();
        gossip.version.clone()
    };

    UdpMessage::Announce(Announce {
        node_addr: Tagged::addr(node_state.identity.addr.clone()),
        capabilities: node_state.identity.capabilities.clone(),
        recipes: node_state
            .identity
            .recipes
            .iter()
            .map(|recipe| recipe.name.clone())
            .collect(),
        peers: peers.into_iter().map(Tagged::addr).collect(),
        version,
    })
}

fn build_ping_from_node(node_state: &Arc<NodeState>) -> UdpMessage {
    let mut last_seen = HashMap::new();
    last_seen.insert(node_state.identity.addr.clone(), now_secs());

    let version = {
        let gossip = node_state.gossip.read().unwrap();
        gossip.version.clone()
    };

    UdpMessage::Ping(Check {
        last_seen: Tagged::last_seen(last_seen),
        version,
    })
}

fn build_pong_from_node(node_state: &Arc<NodeState>) -> UdpMessage {
    let mut last_seen = HashMap::new();
    last_seen.insert(node_state.identity.addr.clone(), now_secs());

    let version = {
        let gossip = node_state.gossip.read().unwrap();
        gossip.version.clone()
    };

    UdpMessage::Pong(Check {
        last_seen: Tagged::last_seen(last_seen),
        version,
    })
}

fn apply_announce_shared(node_state: &Arc<NodeState>, announce: &Announce) {
    let mut gossip = node_state.gossip.write().unwrap();
    let peer = gossip
        .peers
        .entry(announce.node_addr.value.clone())
        .or_insert_with(PeerInfo::unknown);

    peer.capabilities = announce.capabilities.clone();
    peer.recipes = announce.recipes.clone();
    peer.version = announce.version.clone();
    peer.last_seen_us = now_micros();

    for announced_peer in &announce.peers {
        gossip
            .peers
            .entry(announced_peer.value.clone())
            .or_insert_with(PeerInfo::unknown);
    }

    if is_newer_version(&announce.version, &gossip.version) {
        gossip.version = announce.version.clone();
    }
}

fn apply_check_shared(node_state: &Arc<NodeState>, peer_addr: &str, check: &Check) {
    let mut gossip = node_state.gossip.write().unwrap();
    let peer = gossip
        .peers
        .entry(peer_addr.to_string())
        .or_insert_with(PeerInfo::unknown);

    if is_newer_version(&check.version, &peer.version) {
        peer.version = check.version.clone();
    }
    peer.last_seen_us = now_micros();

    if is_newer_version(&check.version, &gossip.version) {
        gossip.version = check.version.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{GossipState as NodeGossipState, Identity, NodeState};
    use std::sync::{Arc, RwLock};

    fn build_shared_state(addr: &str, capabilities: Vec<String>) -> Arc<NodeState> {
        Arc::new(NodeState {
            identity: Identity {
                addr: addr.to_string(),
                capabilities,
                recipes: vec![],
            },
            gossip: RwLock::new(NodeGossipState {
                peers: HashMap::new(),
                version: Version {
                    counter: 1,
                    generation: now_secs(),
                },
            }),
        })
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
    fn handle_udp_message_shared_updates_peer_from_announce() {
        let state = build_shared_state("127.0.0.1:8010", vec!["MakeDough".to_string()]);

        let announce = Announce {
            node_addr: Tagged::addr("127.0.0.1:8012"),
            capabilities: vec!["Bake".to_string()],
            recipes: vec!["Pepperoni".to_string()],
            peers: vec![Tagged::addr("127.0.0.1:8013")],
            version: Version {
                counter: 9,
                generation: now_secs() + 1,
            },
        };

        let response = handle_udp_message_shared(
            &state,
            "127.0.0.1:8012",
            &UdpMessage::Announce(announce.clone()),
        );
        assert!(response.is_none());

        let gossip = state.gossip.read().unwrap();
        let peer = gossip.peers.get("127.0.0.1:8012").unwrap();
        assert_eq!(peer.capabilities, vec!["Bake".to_string()]);
        assert_eq!(peer.recipes, vec!["Pepperoni".to_string()]);
        assert_eq!(peer.version, announce.version);
        assert!(gossip.peers.contains_key("127.0.0.1:8013"));
        assert_eq!(gossip.version, announce.version);
    }

    #[test]
    fn handle_udp_message_shared_ping_returns_pong_and_updates_version() {
        let state = build_shared_state("127.0.0.1:8020", vec!["MakeDough".to_string()]);
        let incoming = Check {
            last_seen: Tagged::last_seen(HashMap::new()),
            version: Version {
                counter: 4,
                generation: now_secs() + 1,
            },
        };

        let response = handle_udp_message_shared(
            &state,
            "127.0.0.1:8022",
            &UdpMessage::Ping(incoming.clone()),
        );

        assert!(matches!(response, Some(UdpMessage::Pong(_))));
        let gossip = state.gossip.read().unwrap();
        let peer = gossip.peers.get("127.0.0.1:8022").unwrap();
        assert_eq!(peer.version, incoming.version);
        assert_eq!(gossip.version, incoming.version);
    }

    #[test]
    fn send_ping_to_known_peers_shared_sends_to_all_neighbors() {
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
        let state = build_shared_state(&sender_addr.to_string(), vec!["MakeDough".to_string()]);

        {
            let mut gossip = state.gossip.write().unwrap();
            gossip.peers.insert(addr_a.to_string(), PeerInfo::unknown());
            gossip.peers.insert(addr_b.to_string(), PeerInfo::unknown());
        }

        let sent = send_ping_to_known_peers_shared(&sender, &state).unwrap();
        assert_eq!(sent, 2);

        let (bytes_a, from_a) = recv_datagram(&receiver_a).unwrap();
        let (bytes_b, from_b) = recv_datagram(&receiver_b).unwrap();

        assert_eq!(from_a, sender_addr);
        assert_eq!(from_b, sender_addr);
        assert!(matches!(
            decode_udp_message(&bytes_a).unwrap(),
            UdpMessage::Ping(_)
        ));
        assert!(matches!(
            decode_udp_message(&bytes_b).unwrap(),
            UdpMessage::Ping(_)
        ));
    }
}
