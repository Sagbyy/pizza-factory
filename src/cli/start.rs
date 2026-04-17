use clap::Args;

fn parse_gossip_rate(value: &str) -> Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|e| format!("invalid gossip-rate '{value}': {e}"))?;

    if (0.0..=1.0).contains(&parsed) {
        Ok(parsed)
    } else {
        Err(format!(
            "invalid gossip-rate '{value}': expected a value between 0.0 and 1.0"
        ))
    }
}

#[derive(Args, Debug)]
pub struct StartArgs {
    #[arg(long, default_value = "127.0.0.1:8000", value_name = "HOST:PORT")]
    pub host: String,
    #[arg(long, value_name = "CAPABILITIES", num_args = 0.., value_delimiter = ',', help = "Capabilities (actions) exposed by this node, comma-separated or repeated")]
    pub capabilities: Vec<String>,
    #[arg(long = "peer", value_name = "PEER:PORT", num_args = 0.., help = "Bootstrap peer(s) to connect to (ip:port). Repeatable")]
    pub peers: Vec<String>,
    #[arg(long, value_name = "RECIPES_FILE", help = "Read recipes from a file")]
    pub recipes_file: Option<String>,
    #[arg(
        long,
        default_value_t = 1.0,
        value_name = "GOSSIP_RATE",
        value_parser = parse_gossip_rate,
        help = "Gossip rate (0.0-1.0)"
    )]
    pub gossip_rate: f64,
    #[arg(long, default_value = "false", help = "Enable debug logging")]
    pub debug: bool,
}
