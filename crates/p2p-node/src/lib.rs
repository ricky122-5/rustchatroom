//! P2P research node implementation with libp2p transports, authenticated identities,
//! distributed hash table routing, GossipSub messaging, Prometheus metrics, and STUN helpers.

pub mod identity;
pub mod metrics;
pub mod net;
pub mod node;

pub use crate::identity::NodeIdentity;
pub use crate::metrics::NodeMetrics;
pub use crate::net::{NodeConfig, NodeError};
pub use crate::node::{GossipMessage, Node};
