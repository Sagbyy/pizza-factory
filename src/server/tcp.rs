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

/// Handle a single client connection.
///
/// `Order` is special: the handler writes `OrderReceipt` mid-execution (frame 1)
/// then returns `CompletedOrder` / `Error` which we write as frame 2.
/// All other commands follow the standard one-frame-in / one-frame-out pattern.
///
/// Errors at any stage are logged; the connection is dropped without crashing
/// the accept loop.
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

    // Order writes frame 1 (OrderReceipt) directly onto the stream, then
    // returns frame 2 (CompletedOrder / Error / OrderDeclined) to us.
    let response = if let TcpMessage::Order { recipe_name } = &msg {
        handlers::handle_order(&state, recipe_name, &mut stream)
    } else {
        dispatch(msg, &state)
    };

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
        TcpMessage::ProcessPayload { payload } => handlers::handle_process_payload(state, payload),
        _ => TcpMessage::Error {
            message: "unexpected message type".into(),
        },
    }
}
