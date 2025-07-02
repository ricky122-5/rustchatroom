use anyhow::Result;
use libp2p::identity::Keypair;
use libp2p::{identity, PeerId};

/// Wrapper around a libp2p secp256k1 identity with helpers for signing payloads.
pub struct NodeIdentity {
    keypair: Keypair,
    pub peer_id: PeerId,
}

impl Clone for NodeIdentity {
    fn clone(&self) -> Self {
        Self::from_keypair(self.keypair.clone())
    }
}

impl NodeIdentity {
    pub fn generate() -> Result<Self> {
        let keypair = Keypair::generate_secp256k1();
        Ok(Self::from_keypair(keypair))
    }

    fn from_keypair(keypair: Keypair) -> Self {
        let peer_id = PeerId::from(keypair.public());
        Self { keypair, peer_id }
    }

    pub fn sign(&self, payload: &[u8]) -> Result<Vec<u8>> {
        Ok(self.keypair.sign(payload)?)
    }

    pub fn public_key(&self) -> identity::PublicKey {
        self.keypair.public()
    }

    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }
}

impl Default for NodeIdentity {
    fn default() -> Self {
        Self::generate().expect("identity generation")
    }
}
