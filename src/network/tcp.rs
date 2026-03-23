use std::io::{Read, Result, Write};
use std::net::TcpStream;

pub fn write_frame(stream: &mut TcpStream, payload: &[u8]) -> Result<()> {
    let len = payload.len() as u32;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(payload)?;
    Ok(())
}

pub fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn test_write_read_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let data = read_frame(&mut conn).unwrap();
            assert_eq!(data, b"hello pizza");
        });

        let mut client = TcpStream::connect(addr).unwrap();
        write_frame(&mut client, b"hello pizza").unwrap();

        handle.join().unwrap();
    }

    #[test]
    fn test_payload_vide() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let data = read_frame(&mut conn).unwrap();
            assert_eq!(data, b"");
        });

        let mut client = TcpStream::connect(addr).unwrap();
        write_frame(&mut client, b"").unwrap();

        handle.join().unwrap();
    }

    #[test]
    fn test_plusieurs_frames() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            assert_eq!(read_frame(&mut conn).unwrap(), b"premier");
            assert_eq!(read_frame(&mut conn).unwrap(), b"deuxieme");
        });

        let mut client = TcpStream::connect(addr).unwrap();
        write_frame(&mut client, b"premier").unwrap();
        write_frame(&mut client, b"deuxieme").unwrap();

        handle.join().unwrap();
    }
}
