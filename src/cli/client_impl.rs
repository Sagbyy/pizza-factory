use std::collections::HashMap;
use std::io;
use std::net::TcpStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::network::tcp::{read_frame, write_frame};
use crate::protocol::{TcpMessage, from_cbor, to_cbor};
use crate::store::{self, Order, OrderStatus, now_ms};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize, Default)]
struct RecipeStatusCompat {
    #[serde(default)]
    missing_actions: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RecipeAvailabilityCompat {
    #[serde(default)]
    local: RecipeStatusCompat,
    #[serde(default)]
    missing_actions: Vec<String>,
    #[serde(default)]
    remote_peers: Vec<String>,
}

impl RecipeAvailabilityCompat {
    fn effective_missing_actions(&self) -> &[String] {
        if !self.local.missing_actions.is_empty() {
            &self.local.missing_actions
        } else {
            &self.missing_actions
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TcpMessageCompat {
    RecipeListAnswer {
        recipes: HashMap<String, RecipeAvailabilityCompat>,
    },
    Error {
        message: String,
    },
}

/// Format the current time as `YYYY-MM-DDTHH:MM:SS.ffffffZ` (UTC, microseconds).
fn now_rfc3339() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let micros = d.subsec_micros();

    let (h, m, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    let days = secs / 86400;

    // Gregorian calendar from days since 1970-01-01
    // Algorithm: http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{micros:06}Z")
}

/// Print a timestamped INFO line to stdout.
fn log_info(msg: &str) {
    println!("{}  INFO {}", now_rfc3339(), msg);
}

fn print_recipe_row(name: &str, missing_actions: &[String], remote_peers: &[String]) {
    if !remote_peers.is_empty() {
        println!("  - {}: available at [{}]", name, remote_peers.join(", "));
    } else if missing_actions.is_empty() {
        println!("  - {}: local (complete)", name);
    } else {
        println!(
            "  - {}: missing actions [{}]",
            name,
            missing_actions.join(", ")
        );
    }
}

/// Connect to a peer and send a ListRecipes request.
pub fn client_list_recipes(peer: &str) -> io::Result<()> {
    let mut stream = TcpStream::connect(peer)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let request = TcpMessage::ListRecipes;
    let request_bytes = to_cbor(&request).map_err(io::Error::other)?;
    write_frame(&mut stream, &request_bytes)?;

    let response_bytes = read_frame(&mut stream)?;

    match from_cbor::<TcpMessage>(&response_bytes) {
        Ok(TcpMessage::RecipeListAnswer { recipes }) => {
            println!("Available recipes:");
            for (name, availability) in recipes {
                print_recipe_row(
                    &name,
                    &availability.local.missing_actions,
                    &availability.remote_peers,
                );
            }
        }
        Ok(TcpMessage::Error { message }) => {
            eprintln!("Error: {}", message);
        }
        Ok(other) => {
            eprintln!("Unexpected response: {:?}", other);
        }
        Err(_) => {
            let compat: TcpMessageCompat = from_cbor(&response_bytes).map_err(io::Error::other)?;
            match compat {
                TcpMessageCompat::RecipeListAnswer { recipes } => {
                    println!("Available recipes:");
                    for (name, availability) in recipes {
                        print_recipe_row(
                            &name,
                            availability.effective_missing_actions(),
                            &availability.remote_peers,
                        );
                    }
                }
                TcpMessageCompat::Error { message } => {
                    eprintln!("Error: {}", message);
                }
            }
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
///
/// Reads two frames:
///   Frame 1 — `OrderReceipt` (acknowledged immediately) or `OrderDeclined` / `Error`.
///   Frame 2 — `CompletedOrder` or `Error` (only when frame 1 was `OrderReceipt`).
pub fn client_order(peer: &str, recipe_name: &str) -> io::Result<()> {
    log_info(&format!("Ordering recipe recipe={recipe_name} peer={peer}"));

    let mut stream = TcpStream::connect(peer)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let request = TcpMessage::Order {
        recipe_name: recipe_name.to_string(),
    };
    let request_bytes = to_cbor(&request).map_err(io::Error::other)?;
    write_frame(&mut stream, &request_bytes)?;

    // Frame 1: immediate acknowledgement.
    let frame1_bytes = read_frame(&mut stream)?;
    let frame1: TcpMessage = from_cbor(&frame1_bytes).map_err(io::Error::other)?;

    let local_id = Uuid::new_v4().as_u128();
    store::add_order(Order {
        id: local_id,
        server_id: None,
        recipe_name: recipe_name.to_string(),
        status: OrderStatus::Sending,
        timestamp_ms: now_ms(),
    });

    match frame1 {
        TcpMessage::OrderReceipt { order_id } => {
            log_info(&format!(
                "Order receipt {}; waiting for completion...",
                order_id.0
            ));
            store::update_order_server_id(local_id, &order_id.0);
            store::update_order_status(local_id, OrderStatus::Receipt);
        }
        TcpMessage::OrderDeclined { message } => {
            log_info(&format!("Order of '{recipe_name}' declined: {message}"));
            store::update_order_status(local_id, OrderStatus::Declined(message));
            return Ok(());
        }
        TcpMessage::Error { message } => {
            log_info(&format!("Error: {message}"));
            store::update_order_status(local_id, OrderStatus::Error(message));
            return Ok(());
        }
        other => {
            log_info(&format!("Unexpected response: {other:?}"));
            return Ok(());
        }
    }

    // Frame 2: execution result.
    let frame2_bytes = read_frame(&mut stream)?;
    let frame2: TcpMessage = from_cbor(&frame2_bytes).map_err(io::Error::other)?;

    match frame2 {
        TcpMessage::CompletedOrder {
            recipe_name,
            result,
        } => {
            log_info("Order completed successfully");
            log_info(&format!("Recipe {recipe_name}:"));
            println!("{result}");
            store::update_order_status(local_id, OrderStatus::Delivered);
        }
        TcpMessage::FailedOrder { recipe_name, error } => {
            log_info(&format!("Order of '{recipe_name}' failed: {error}"));
            store::update_order_status(local_id, OrderStatus::Failed(error));
        }
        TcpMessage::Error { message } => {
            log_info(&format!("Execution error: {message}"));
            store::update_order_status(local_id, OrderStatus::Error(message));
        }
        other => {
            log_info(&format!("Unexpected result: {other:?}"));
        }
    }

    Ok(())
}
