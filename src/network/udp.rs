use std::io;
use std::net::UdpSocket;

pub fn send_datagram(socket: &UdpSocket, payload: &[u8], target: &str) -> io::Result<()> {
    socket.send_to(payload, target)?;
    Ok(())
}

pub fn recv_datagram(socket: &UdpSocket) -> io::Result<(Vec<u8>, std::net::SocketAddr)> {
    let mut buf = vec![0u8; 65535];
    let (len, addr) = socket.recv_from(&mut buf)?;
    buf.truncate(len);
    Ok((buf, addr))
}
