use std::io::Result;
use std::net::{SocketAddr, UdpSocket};

use crate::protocol::{UdpMessage, Version, from_cbor, to_cbor};

/// Sends a raw UDP datagram to a target address.
pub fn send_datagram(socket: &UdpSocket, payload: &[u8], target: &str) -> Result<()> {
    socket.send_to(payload, target)?;
    Ok(())
}

/// Receives one UDP datagram and returns `(payload, source_addr)`.
pub fn recv_datagram(socket: &UdpSocket) -> Result<(Vec<u8>, SocketAddr)> {
    let mut buf = vec![0u8; 65535];
    let (len, addr) = socket.recv_from(&mut buf)?;
    buf.truncate(len);
    Ok((buf, addr))
}

/// Encodes a UDP protocol message into CBOR bytes.
pub fn encode_udp_message(message: &UdpMessage) -> Result<Vec<u8>> {
    to_cbor(message).map_err(std::io::Error::other)
}

/// Decodes a CBOR payload into a UDP protocol message.
pub fn decode_udp_message(bytes: &[u8]) -> Result<UdpMessage> {
    from_cbor(bytes).map_err(std::io::Error::other)
}

/// Encodes and sends a UDP protocol message to a target address.
pub fn send_udp_message(socket: &UdpSocket, message: &UdpMessage, target: &str) -> Result<()> {
    let payload = encode_udp_message(message)?;
    send_datagram(socket, &payload, target)
}

/// Returns `true` when `candidate` is newer than `current`.
pub fn is_newer_version(candidate: &Version, current: &Version) -> bool {
    (candidate.generation, candidate.counter) > (current.generation, current.counter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Announce, Check, LastSeenMap};
    use std::collections::HashMap;

    #[test]
    fn is_newer_version_when_generation_is_greater() {
        let current = Version {
            generation: 1000,
            counter: 5,
        };
        let newer = Version {
            generation: 2000,
            counter: 1,
        };
        assert!(is_newer_version(&newer, &current));
        assert!(!is_newer_version(&current, &newer));
    }

    #[test]
    fn is_newer_version_when_generation_equal_and_counter_greater() {
        let current = Version {
            generation: 1000,
            counter: 5,
        };
        let newer = Version {
            generation: 1000,
            counter: 10,
        };
        assert!(is_newer_version(&newer, &current));
        assert!(!is_newer_version(&current, &newer));
    }

    #[test]
    fn is_newer_version_when_equal() {
        let ver1 = Version {
            generation: 1000,
            counter: 5,
        };
        let ver2 = Version {
            generation: 1000,
            counter: 5,
        };
        assert!(!is_newer_version(&ver1, &ver2));
        assert!(!is_newer_version(&ver2, &ver1));
    }

    #[test]
    fn is_newer_version_when_generation_smaller() {
        let current = Version {
            generation: 2000,
            counter: 100,
        };
        let older = Version {
            generation: 1000,
            counter: 1000,
        };
        assert!(!is_newer_version(&older, &current));
        assert!(is_newer_version(&current, &older));
    }

    #[test]
    fn encode_decode_announce_roundtrip() -> Result<()> {
        let message = UdpMessage::Announce(Announce {
            node_addr: crate::protocol::addr("127.0.0.1:9000"),
            capabilities: vec!["Bake".to_string(), "MakeDough".to_string()],
            recipes: vec!["Margherita".to_string()],
            peers: vec![crate::protocol::addr("127.0.0.1:9001")],
            version: Version {
                generation: 5555,
                counter: 42,
            },
        });

        let encoded = encode_udp_message(&message)?;
        let decoded = decode_udp_message(&encoded)?;

        match decoded {
            UdpMessage::Announce(ann) => {
                assert_eq!(ann.node_addr.0, "127.0.0.1:9000");
                assert_eq!(ann.capabilities, vec!["Bake", "MakeDough"]);
                assert_eq!(ann.recipes, vec!["Margherita"]);
                assert_eq!(ann.peers.len(), 1);
                assert_eq!(ann.version.generation, 5555);
                assert_eq!(ann.version.counter, 42);
            }
            _ => panic!("expected Announce, got {:?}", decoded),
        }

        Ok(())
    }

    #[test]
    fn encode_decode_ping_roundtrip() -> Result<()> {
        let mut by_code = HashMap::new();
        by_code.insert(1_i64, 1234567890u64);
        by_code.insert(-6_i64, 654321u64);

        let message = UdpMessage::Ping(Check {
            last_seen: ciborium::tag::Required(LastSeenMap::ByCode(by_code.clone())),
            version: Version {
                generation: 2022,
                counter: 7,
            },
        });

        let encoded = encode_udp_message(&message)?;
        let decoded = decode_udp_message(&encoded)?;

        match decoded {
            UdpMessage::Ping(check) => {
                assert_eq!(check.version.generation, 2022);
                assert_eq!(check.version.counter, 7);
                match check.last_seen.0 {
                    LastSeenMap::ByCode(m) => {
                        assert_eq!(m.get(&1), Some(&1234567890u64));
                        assert_eq!(m.get(&-6), Some(&654321u64));
                    }
                    _ => panic!("expected numeric last_seen"),
                }
            }
            _ => panic!("expected Ping, got {:?}", decoded),
        }

        Ok(())
    }

    #[test]
    fn decode_invalid_cbor_returns_error() {
        let invalid_bytes = vec![0xFF, 0xFF, 0xFF];
        let result = decode_udp_message(&invalid_bytes);
        assert!(result.is_err());
    }

    #[test]
    fn decode_empty_payload_returns_error() {
        let result = decode_udp_message(&[][..]);
        assert!(result.is_err());
    }
}
