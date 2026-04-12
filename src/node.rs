use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::start::StartArgs;
use crate::protocol::Version;
use crate::recipe::{parse_recipes, ParseError, Recipe};

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum NodeError {
    RecipesFileRead { path: String, cause: std::io::Error },
    RecipesFileParse { path: String, cause: ParseError },
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeError::RecipesFileRead { path, .. } => {
                write!(f, "Failed to read recipes file: {path}")
            }
            NodeError::RecipesFileParse { path, .. } => {
                write!(f, "Failed to parse recipes file: {path}")
            }
        }
    }
}

impl std::error::Error for NodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            NodeError::RecipesFileRead { cause, .. } => Some(cause),
            NodeError::RecipesFileParse { cause, .. } => Some(cause),
        }
    }
}

// ── Identity ──────────────────────────────────────────────────────────────────
// Set once at startup from CLI args. Never mutated. No lock needed.

pub struct Identity {
    /// Own UDP/TCP address, e.g. "127.0.0.1:8000".
    pub addr: String,
    /// Actions this node can execute, e.g. ["MakeDough"].
    pub capabilities: Vec<String>,
    /// Full recipes loaded from --recipes-file, or empty if no file given.
    pub recipes: Vec<Recipe>,
}

// ── GossipState ───────────────────────────────────────────────────────────────
// Written by the UDP layer (Announce / Pong). Read by TCP for order routing.

pub struct GossipState {
    /// Known peers, keyed by their address string.
    pub peers: HashMap<String, PeerInfo>,
    /// Own version vector, incremented by the UDP sender on each Announce.
    pub version: Version,
}

/// What is known about a remote peer, updated on each Announce / Pong received.
pub struct PeerInfo {
    /// Capabilities advertised by that peer.
    pub capabilities: Vec<String>,
    /// Recipe names known to that peer (names only, not full recipes).
    pub recipes: Vec<String>,
    /// Last version vector received from that peer.
    pub version: Version,
    /// Unix timestamp in microseconds of the last message received from that peer.
    pub last_seen_us: u64,
}

impl PeerInfo {
    /// Placeholder for a bootstrap peer whose capabilities are not yet known.
    pub fn unknown() -> Self {
        PeerInfo {
            capabilities: vec![],
            recipes: vec![],
            version: Version { counter: 0, generation: 0 },
            last_seen_us: 0,
        }
    }
}

// ── NodeState ─────────────────────────────────────────────────────────────────

pub struct NodeState {
    /// Immutable identity — no lock, direct field access.
    pub identity: Identity,
    /// Mutable gossip state — owned by the UDP layer, read by TCP.
    pub gossip: RwLock<GossipState>,
}

impl NodeState {
    /// Build the shared node state from CLI arguments and wrap it in `Arc`.
    ///
    /// Returns `Result` so startup errors (bad recipes file) are reported
    /// cleanly to the caller instead of panicking.
    pub fn new(args: &StartArgs) -> Result<Arc<Self>, NodeError> {
        let identity = Identity {
            addr: args.host.clone(),
            capabilities: parse_capabilities(&args.capabilities),
            recipes: load_recipes(args.recipes_file.as_deref())?,
        };

        let gossip = GossipState {
            peers: bootstrap_peers(&args.peers),
            version: Version {
                counter: 0,
                generation: unix_secs(),
            },
        };

        Ok(Arc::new(NodeState {
            identity,
            gossip: RwLock::new(gossip),
        }))
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// The CLI accepts capabilities either as repeated flags or comma-separated:
///   --capabilities MakeDough
///   --capabilities AddBase,AddCheese,Bake
/// Both forms arrive as Vec<String>; split on ',' to normalise.
fn parse_capabilities(raw: &[String]) -> Vec<String> {
    raw.iter()
        .flat_map(|s| s.split(','))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Read and parse the recipes file. Returns an empty vec if no file is given.
/// Propagates I/O and parse errors as `NodeError` so the caller decides how to handle them.
fn load_recipes(path: Option<&str>) -> Result<Vec<Recipe>, NodeError> {
    let Some(path) = path else { return Ok(vec![]) };
    let content = std::fs::read_to_string(path).map_err(|e| NodeError::RecipesFileRead {
        path: path.to_string(),
        cause: e,
    })?;
    parse_recipes(&content).map_err(|e| NodeError::RecipesFileParse {
        path: path.to_string(),
        cause: e,
    })
}

/// Seed the peer table from --peer bootstrap addresses.
/// Capabilities are not yet known; they will be filled in by the UDP layer
/// when the first Announce is received from each peer.
fn bootstrap_peers(addrs: &[String]) -> HashMap<String, PeerInfo> {
    addrs.iter()
        .map(|addr| (addr.clone(), PeerInfo::unknown()))
        .collect()
}

/// Current time as Unix seconds — used as the `generation` field in Version,
/// matching the spec values (e.g. 1772191739).
fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
