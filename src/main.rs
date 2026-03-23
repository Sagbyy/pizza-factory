mod cli;
mod network;
mod protocol;

use crate::cli::parse_args;

fn main() {
    let args = parse_args();

    println!("Hello, world!");
}
