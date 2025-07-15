use anyhow::{anyhow, Result};
use libp2p_identity::{self as identity, Keypair, PeerId, PublicKey};

/// Wrapper around a libp2p identity with helpers for authenticating payloads via secp256k1
/// signatures while remaining compatible with libp2p transports.
pub struct NodeIdentity {
    keypair: Keypair,
    pub peer_id: PeerId,
    secp: identity::secp256k1::Keypair,
}

impl Clone for NodeIdentity {
    fn clone(&self) -> Self {
        Self {
            keypair: self.keypair.clone(),
            peer_id: self.peer_id,
            secp: self.secp.clone(),
        }
    }
}

impl NodeIdentity {
    pub fn generate() -> Result<Self> {
        let secp = identity::secp256k1::Keypair::generate();
        Self::from_secp256k1(secp)
    }

    pub fn from_secp256k1(secp: identity::secp256k1::Keypair) -> Result<Self> {
        let keypair = Keypair::from(secp.clone());
        let peer_id = keypair.public().to_peer_id();
        Ok(Self {
            keypair,
            peer_id,
            secp,
        })
    }

    pub fn from_keypair_bytes(bytes: &[u8]) -> Result<Self> {
        let keypair =
            Keypair::from_protobuf_encoding(bytes).map_err(|e| anyhow!("decode keypair: {e}"))?;
        let secp = keypair
            .clone()
            .try_into_secp256k1()
            .map_err(|e| anyhow!("keypair is not secp256k1: {e}"))?;
        let peer_id = keypair.public().to_peer_id();
        Ok(Self {
            keypair,
            peer_id,
            secp,
        })
    }

    pub fn sign(&self, payload: &[u8]) -> Result<Vec<u8>> {
        Ok(self.secp.secret().sign(payload))
    }

    pub fn public_key(&self) -> PublicKey {
        self.keypair.public()
    }

    pub fn public_key_protobuf(&self) -> Vec<u8> {
        self.public_key().encode_protobuf()
    }

    pub fn secp_public_key_bytes(&self) -> Vec<u8> {
        self.secp.public().to_bytes().to_vec()
    }

    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }

    pub fn to_keypair_bytes(&self) -> Result<Vec<u8>> {
        self.keypair
            .to_protobuf_encoding()
            .map_err(|e| anyhow!("encode keypair: {e}"))
    }
}

impl Default for NodeIdentity {
    fn default() -> Self {
        Self::generate().expect("identity generation")
    }
}
