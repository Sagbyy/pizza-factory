use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use crate::network::tcp::{read_frame, write_frame};
use crate::node::NodeState;
use crate::protocol::{TcpMessage, from_cbor, to_cbor};
use crate::server::handlers;

/// Bind the TCP listener and spawn the accept loop thread.
///
/// Returns the join handle so the caller can block on it.
/// Returns `Err` immediately if the bind fails (port already in use, bad address, etc.).
pub fn start(state: Arc<NodeState>) -> Result<thread::JoinHandle<()>, io::Error> {
    let listener = TcpListener::bind(&state.identity.addr)?;

    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let state = Arc::clone(&state);
                    thread::spawn(move || handle_connection(stream, state));
                }
                Err(e) => {
                    eprintln!("TCP accept error: {e}");
                }
            }
        }
    });

    Ok(handle)
}

/// Handle a single client connection: one frame in, one frame out, then close.
///
/// Errors at any stage (read, decode, encode, write) are logged and the
/// connection is dropped — they must not crash the accept loop.
fn handle_connection(mut stream: TcpStream, state: Arc<NodeState>) {
    let bytes = match read_frame(&mut stream) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("TCP read error: {e}");
            return;
        }
    };

    let msg: TcpMessage = match from_cbor(&bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("TCP decode error: {e}");
            return;
        }
    };

    let response = dispatch(msg, &state);

    let response_bytes = match to_cbor(&response) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("TCP encode error: {e}");
            return;
        }
    };

    if let Err(e) = write_frame(&mut stream, &response_bytes) {
        eprintln!("TCP write error: {e}");
    }
}

/// Route a decoded `TcpMessage` to the appropriate handler.
fn dispatch(msg: TcpMessage, state: &NodeState) -> TcpMessage {
    match msg {
        TcpMessage::ListRecipes => handlers::handle_list_recipes(state),
        TcpMessage::GetRecipe { recipe_name } => handlers::handle_get_recipe(state, &recipe_name),
        TcpMessage::Order { recipe_name } => handlers::handle_order(state, &recipe_name),
        TcpMessage::ProcessPayload { payload } => handlers::handle_process_payload(state, payload),
        _ => TcpMessage::Error {
            message: "unexpected message type".into(),
        },
    }
}
