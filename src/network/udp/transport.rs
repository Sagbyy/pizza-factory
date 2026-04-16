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
