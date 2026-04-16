use std::collections::HashMap;
use std::io::Result;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::node::{NodeState, PeerInfo};
use crate::protocol::{Announce, Check, LastSeenMap, UdpMessage, Version};

use super::transport::{decode_udp_message, is_newer_version, recv_datagram, send_udp_message};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn heartbeat_last_seen_now() -> crate::protocol::TaggedLastSeen {
    let micros = now_micros();
    let secs = micros / 1_000_000;
    let frac = micros % 1_000_000;

    let mut by_code = HashMap::new();
    by_code.insert(1_i64, secs);
    by_code.insert(-6_i64, frac);

    ciborium::tag::Required(LastSeenMap::ByCode(by_code))
}

/// Runs the shared UDP gossip service loop for a node.
///
/// Sends an initial `Announce` to bootstrap peers, then continuously executes
/// one heartbeat tick every 2 seconds.
pub fn run_gossip_service_shared(
    socket: &std::net::UdpSocket,
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

/// Executes one gossip iteration:
///
/// 1. sends pings,
/// 2. processes incoming datagrams until the next heartbeat slot.
pub fn gossip_tick_shared(
    socket: &std::net::UdpSocket,
    node_state: &Arc<NodeState>,
) -> Result<UdpMessage> {
    let _ = send_ping_to_known_peers_shared(socket, node_state)?;

    let started = Instant::now();
    let mut last_message = UdpMessage::Pong(Check {
        last_seen: heartbeat_last_seen_now(),
        version: Version {
            counter: 0,
            generation: 0,
        },
    });

    while started.elapsed() < HEARTBEAT_INTERVAL {
        last_message = process_one_datagram_shared(socket, node_state)?;
    }

    Ok(last_message)
}

/// Sends a `Ping` message to every known peer except self.
///
/// Returns the number of peers that were successfully targeted.
pub fn send_ping_to_known_peers_shared(
    socket: &std::net::UdpSocket,
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

/// Reads and handles a single incoming UDP datagram.
///
/// On timeout / would-block, returns a placeholder `Pong` so the caller can
/// continue looping without treating it as a fatal error.
pub fn process_one_datagram_shared(
    socket: &std::net::UdpSocket,
    node_state: &Arc<NodeState>,
) -> Result<UdpMessage> {
    match recv_datagram(socket) {
        Ok((bytes, from)) => {
            let message = match decode_udp_message(&bytes) {
                Ok(msg) => msg,
                Err(e) => {
                    eprintln!("UDP decode error from {from}: {e}");
                    return Ok(UdpMessage::Pong(Check {
                        last_seen: heartbeat_last_seen_now(),
                        version: Version {
                            counter: 0,
                            generation: 0,
                        },
                    }));
                }
            };
            let from_addr = from.to_string();
            if let Some(reply) = handle_udp_message_shared(node_state, &from_addr, &message) {
                send_udp_message(socket, &reply, &from_addr)?;
            }

            Ok(message)
        }
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut
                || e.kind() == std::io::ErrorKind::ConnectionReset =>
        {
            Ok(UdpMessage::Pong(Check {
                last_seen: heartbeat_last_seen_now(),
                version: Version {
                    counter: 0,
                    generation: 0,
                },
            }))
        }
        Err(e) => Err(e),
    }
}

/// Applies one UDP message to shared node state and optionally builds a reply.
///
/// - `Announce` updates known peer data and replies with own `Announce` if peer is newly discovered.
/// - `Ping` updates liveness/version data and returns a `Pong` reply.
/// - `Pong` only updates liveness/version data.
pub fn handle_udp_message_shared(
    node_state: &Arc<NodeState>,
    peer_addr: &str,
    message: &UdpMessage,
) -> Option<UdpMessage> {
    match message {
        UdpMessage::Announce(announce) => {
            let is_first_announce = apply_announce_shared(node_state, announce);
            if is_first_announce {
                let peer_addrs = {
                    let gossip = node_state.gossip.read().unwrap();
                    gossip.peers.keys().cloned().collect()
                };
                Some(build_announce_from_node(node_state, peer_addrs))
            } else {
                None
            }
        }
        UdpMessage::Ping(check) => {
            apply_check_shared(node_state, peer_addr, check);
            Some(build_pong_from_node(node_state, check.last_seen.clone()))
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
        node_addr: crate::protocol::addr(node_state.identity.addr.clone()),
        capabilities: node_state.identity.capabilities.clone(),
        recipes: node_state
            .identity
            .recipes
            .iter()
            .map(|recipe| recipe.name.clone())
            .collect(),
        peers: peers.into_iter().map(crate::protocol::addr).collect(),
        version,
    })
}

fn build_ping_from_node(node_state: &Arc<NodeState>) -> UdpMessage {
    let version = {
        let gossip = node_state.gossip.read().unwrap();
        gossip.version.clone()
    };

    UdpMessage::Ping(Check {
        last_seen: heartbeat_last_seen_now(),
        version,
    })
}

fn build_pong_from_node(
    node_state: &Arc<NodeState>,
    echoed_last_seen: crate::protocol::TaggedLastSeen,
) -> UdpMessage {
    let version = {
        let gossip = node_state.gossip.read().unwrap();
        gossip.version.clone()
    };

    UdpMessage::Pong(Check {
        last_seen: echoed_last_seen,
        version,
    })
}

fn apply_announce_shared(node_state: &Arc<NodeState>, announce: &Announce) -> bool {
    let mut gossip = node_state.gossip.write().unwrap();
    let is_new_peer = !gossip.peers.contains_key(&announce.node_addr.0);
    let peer = gossip
        .peers
        .entry(announce.node_addr.0.clone())
        .or_insert_with(PeerInfo::unknown);

    peer.capabilities = announce.capabilities.clone();
    peer.recipes = announce.recipes.clone();
    peer.version = announce.version.clone();
    peer.last_seen_us = now_micros();

    for announced_peer in &announce.peers {
        gossip
            .peers
            .entry(announced_peer.0.clone())
            .or_insert_with(PeerInfo::unknown);
    }

    if is_newer_version(&announce.version, &gossip.version) {
        gossip.version = announce.version.clone();
    }

    is_new_peer
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
    use std::sync::{Arc, RwLock};

    use super::super::transport::encode_udp_message;
    use crate::node::{GossipState as NodeGossipState, Identity, NodeState};
    use crate::protocol::{LastSeenMap, UdpMessage, Version};

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
                    generation: 1000,
                },
            }),
        })
    }

    #[test]
    fn udp_message_roundtrip_over_helpers() {
        let message = UdpMessage::Announce(Announce {
            node_addr: crate::protocol::addr("127.0.0.1:8000"),
            capabilities: vec!["MakeDough".to_string()],
            recipes: vec!["Margherita".to_string()],
            peers: vec![crate::protocol::addr("127.0.0.1:8002")],
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
            node_addr: crate::protocol::addr("127.0.0.1:8012"),
            capabilities: vec!["Bake".to_string()],
            recipes: vec!["Pepperoni".to_string()],
            peers: vec![crate::protocol::addr("127.0.0.1:8013")],
            version: Version {
                counter: 9,
                generation: 1001,
            },
        };

        let response = handle_udp_message_shared(
            &state,
            "127.0.0.1:8012",
            &UdpMessage::Announce(announce.clone()),
        );
        assert!(matches!(response, Some(UdpMessage::Announce(_))));

        let gossip = state.gossip.read().unwrap();
        let peer = gossip.peers.get("127.0.0.1:8012").unwrap();
        assert_eq!(peer.capabilities, vec!["Bake".to_string()]);
        assert_eq!(peer.recipes, vec!["Pepperoni".to_string()]);
        assert_eq!(peer.version, announce.version);
        assert!(gossip.peers.contains_key("127.0.0.1:8013"));
        assert_eq!(gossip.version, announce.version);
    }

    #[test]
    fn handle_udp_message_shared_known_peer_announce_does_not_reply() {
        let state = build_shared_state("127.0.0.1:8040", vec!["MakeDough".to_string()]);
        {
            let mut gossip = state.gossip.write().unwrap();
            gossip
                .peers
                .insert("127.0.0.1:8042".to_string(), PeerInfo::unknown());
        }

        let announce = Announce {
            node_addr: crate::protocol::addr("127.0.0.1:8042"),
            capabilities: vec!["Bake".to_string()],
            recipes: vec!["Pepperoni".to_string()],
            peers: vec![crate::protocol::addr("127.0.0.1:8043")],
            version: Version {
                counter: 10,
                generation: 1001,
            },
        };

        let response = handle_udp_message_shared(
            &state,
            "127.0.0.1:8042",
            &UdpMessage::Announce(announce.clone()),
        );

        assert!(response.is_none());

        let gossip = state.gossip.read().unwrap();
        let peer = gossip.peers.get("127.0.0.1:8042").unwrap();
        assert_eq!(peer.capabilities, vec!["Bake".to_string()]);
        assert_eq!(peer.recipes, vec!["Pepperoni".to_string()]);
        assert_eq!(peer.version, announce.version);
    }

    #[test]
    fn handle_udp_message_shared_ping_returns_pong_and_updates_version() {
        let state = build_shared_state("127.0.0.1:8020", vec!["MakeDough".to_string()]);
        let incoming = Check {
            last_seen: ciborium::tag::Required(LastSeenMap::ByCode(HashMap::new())),
            version: Version {
                counter: 4,
                generation: 1001,
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
        let receiver_a = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver_a
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let addr_a = receiver_a.local_addr().unwrap();

        let receiver_b = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver_b
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let addr_b = receiver_b.local_addr().unwrap();

        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
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
        let ping_a = decode_udp_message(&bytes_a).unwrap();
        let ping_b = decode_udp_message(&bytes_b).unwrap();
        assert!(matches!(ping_a, UdpMessage::Ping(_)));
        assert!(matches!(ping_b, UdpMessage::Ping(_)));

        let assert_numeric_last_seen = |message: UdpMessage| match message {
            UdpMessage::Ping(Check { last_seen, .. }) => match last_seen.0 {
                LastSeenMap::ByCode(m) => {
                    assert!(m.contains_key(&1));
                    assert!(m.contains_key(&-6));
                }
                LastSeenMap::ByAddress(_) => {
                    panic!("expected numeric-keyed last_seen in Ping")
                }
            },
            _ => panic!("expected Ping"),
        };

        assert_numeric_last_seen(ping_a);
        assert_numeric_last_seen(ping_b);
    }

    #[test]
    fn handle_udp_message_shared_ping_echoes_last_seen_in_pong() {
        let state = build_shared_state("127.0.0.1:8030", vec!["MakeDough".to_string()]);

        let mut by_code = HashMap::new();
        by_code.insert(1_i64, 1_776_203_464);
        by_code.insert(-6_i64, 732_948);
        let incoming = Check {
            last_seen: ciborium::tag::Required(LastSeenMap::ByCode(by_code.clone())),
            version: Version {
                counter: 2,
                generation: 1000,
            },
        };

        let response = handle_udp_message_shared(
            &state,
            "127.0.0.1:8031",
            &UdpMessage::Ping(incoming.clone()),
        );

        match response {
            Some(UdpMessage::Pong(Check { last_seen, .. })) => {
                assert_eq!(last_seen.0, LastSeenMap::ByCode(by_code));
            }
            other => panic!("expected Pong response, got {other:?}"),
        }
    }

    #[test]
    fn process_one_datagram_shared_announce_from_new_peer_emits_reply() {
        let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let receiver_addr = receiver.local_addr().unwrap();

        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let sender_addr = sender.local_addr().unwrap();

        let state = build_shared_state(&receiver_addr.to_string(), vec!["MakeDough".to_string()]);

        let announce = UdpMessage::Announce(Announce {
            node_addr: crate::protocol::addr(sender_addr.to_string()),
            capabilities: vec!["Bake".to_string()],
            recipes: vec!["Pepperoni".to_string()],
            peers: vec![crate::protocol::addr(receiver_addr.to_string())],
            version: Version {
                counter: 1,
                generation: 1001,
            },
        });

        let payload = encode_udp_message(&announce).unwrap();
        sender.send_to(&payload, receiver_addr).unwrap();

        let processed = process_one_datagram_shared(&receiver, &state).unwrap();
        assert!(matches!(processed, UdpMessage::Announce(_)));

        let mut buf = [0u8; 2048];
        let (len, _) = sender
            .recv_from(&mut buf)
            .unwrap_or_else(|_| (0, sender_addr));
        assert!(len > 0, "Expected Announce reply from receiver");
    }

    #[test]
    fn process_one_datagram_shared_second_announce_no_reply() {
        let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let receiver_addr = receiver.local_addr().unwrap();

        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .set_read_timeout(Some(std::time::Duration::from_millis(200)))
            .unwrap();
        let sender_addr = sender.local_addr().unwrap();

        let state = build_shared_state(&receiver_addr.to_string(), vec!["MakeDough".to_string()]);

        let announce = UdpMessage::Announce(Announce {
            node_addr: crate::protocol::addr(sender_addr.to_string()),
            capabilities: vec!["Bake".to_string()],
            recipes: vec!["Pepperoni".to_string()],
            peers: vec![crate::protocol::addr(receiver_addr.to_string())],
            version: Version {
                counter: 1,
                generation: 1001,
            },
        });

        let payload = encode_udp_message(&announce).unwrap();

        sender.send_to(&payload, receiver_addr).unwrap();
        let _ = process_one_datagram_shared(&receiver, &state).unwrap();

        let mut buf = [0u8; 2048];
        let _ = sender.recv_from(&mut buf);

        sender.send_to(&payload, receiver_addr).unwrap();
        let _ = process_one_datagram_shared(&receiver, &state).unwrap();

        let err = sender.recv_from(&mut buf).unwrap_err();
        assert!(matches!(
            err.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
    }
}
