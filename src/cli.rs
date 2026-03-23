use clap::Parser;

#[derive(Parser)]
#[command(name = "pizza-factory")]
#[command(about = "Decentralized Pizza Factory")]
#[command(version = "1.0")]
pub struct Args {
    host: String,
    port: u16,
    recipe: String,
    action: String,
    content: String,
}

pub fn parse_args() -> Args {
    let args = Args::parse();
    args
}
