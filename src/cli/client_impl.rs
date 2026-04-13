use std::io;
use std::net::TcpStream;
use std::time::Duration;

use crate::network::tcp::{read_frame, write_frame};
use crate::protocol::{TcpMessage, from_cbor, to_cbor};

/// Connect to a peer and send a ListRecipes request.
pub fn client_list_recipes(peer: &str) -> io::Result<()> {
    let mut stream = TcpStream::connect(peer)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let request = TcpMessage::ListRecipes;
    let request_bytes = to_cbor(&request).map_err(io::Error::other)?;
    write_frame(&mut stream, &request_bytes)?;

    let response_bytes = read_frame(&mut stream)?;
    let response: TcpMessage = from_cbor(&response_bytes).map_err(io::Error::other)?;

    match response {
        TcpMessage::RecipeListAnswer { recipes } => {
            println!("Available recipes:");
            for (name, availability) in recipes {
                if !availability.remote_peers.is_empty() {
                    println!(
                        "  - {}: available at [{}]",
                        name,
                        availability.remote_peers.join(", ")
                    );
                } else if availability.local.missing_actions.is_empty() {
                    println!("  - {}: local (complete)", name);
                } else {
                    println!(
                        "  - {}: missing actions [{}]",
                        name,
                        availability.local.missing_actions.join(", ")
                    );
                }
            }
        }
        TcpMessage::Error { message } => {
            eprintln!("Error: {}", message);
        }
        _ => {
            eprintln!("Unexpected response: {:?}", response);
        }
    }

    Ok(())
}

/// Connect to a peer and send a GetRecipe request.
pub fn client_get_recipe(peer: &str, recipe_name: &str) -> io::Result<()> {
    let mut stream = TcpStream::connect(peer)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let request = TcpMessage::GetRecipe {
        recipe_name: recipe_name.to_string(),
    };
    let request_bytes = to_cbor(&request).map_err(io::Error::other)?;
    write_frame(&mut stream, &request_bytes)?;

    let response_bytes = read_frame(&mut stream)?;
    let response: TcpMessage = from_cbor(&response_bytes).map_err(io::Error::other)?;

    match response {
        TcpMessage::RecipeAnswer { recipe } => {
            println!("Recipe '{}' source:\n{}", recipe_name, recipe);
        }
        TcpMessage::Error { message } => {
            eprintln!("Error: {}", message);
        }
        _ => {
            eprintln!("Unexpected response: {:?}", response);
        }
    }

    Ok(())
}

/// Connect to a peer and send an Order request.
pub fn client_order(peer: &str, recipe_name: &str) -> io::Result<()> {
    let mut stream = TcpStream::connect(peer)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let request = TcpMessage::Order {
        recipe_name: recipe_name.to_string(),
    };
    let request_bytes = to_cbor(&request).map_err(io::Error::other)?;
    write_frame(&mut stream, &request_bytes)?;

    let response_bytes = read_frame(&mut stream)?;
    let response: TcpMessage = from_cbor(&response_bytes).map_err(io::Error::other)?;

    match response {
        TcpMessage::OrderReceipt { order_id } => {
            println!("Order placed successfully! Order ID: {}", order_id.0);
        }
        TcpMessage::Error { message } => {
            eprintln!("Error: {}", message);
        }
        _ => {
            eprintln!("Unexpected response: {:?}", response);
        }
    }

    Ok(())
}
