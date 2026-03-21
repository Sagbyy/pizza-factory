use std::io;
use std::net::{SocketAddr, UdpSocket};

pub fn send_datagram(socket: &UdpSocket, payload: &[u8], target: &str) -> io::Result<()> {
    socket.send_to(payload, target)?;
    Ok(())
}

pub fn recv_datagram(socket: &UdpSocket) -> io::Result<(Vec<u8>, SocketAddr)> {
    let mut buf = vec![0u8; 65535];
    let (len, addr) = socket.recv_from(&mut buf)?;
    buf.truncate(len);
    Ok((buf, addr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_recv_datagram() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = receiver.local_addr().unwrap();

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        send_datagram(&sender, b"announce pizza", &addr.to_string()).unwrap();

        let (data, _) = recv_datagram(&receiver).unwrap();
        assert_eq!(data, b"announce pizza");
    }

    #[test]
    fn test_adresse_expediteur() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let recv_addr = receiver.local_addr().unwrap();

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_addr = sender.local_addr().unwrap();

        send_datagram(&sender, b"ping", &recv_addr.to_string()).unwrap();

        let (data, from) = recv_datagram(&receiver).unwrap();
        assert_eq!(data, b"ping");
        assert_eq!(from, sender_addr);
    }
}
