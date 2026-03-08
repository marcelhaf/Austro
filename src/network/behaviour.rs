use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use libp2p::{
    gossipsub::{self, Behaviour as Gossipsub, MessageId},
    identity::Keypair,
    mdns::{tokio::Behaviour as Mdns, Config as MdnsConfig},
    PeerId,
};
use libp2p_swarm::NetworkBehaviour;

pub const TOPIC_BLOCKS: &str = "austro-blocks";
pub const TOPIC_TRANSACTIONS: &str = "austro-transactions";

#[derive(NetworkBehaviour)]
pub struct AustroBehaviour {
    pub gossipsub: Gossipsub,
    pub mdns: Mdns,
}

impl AustroBehaviour {
    // Now receives the swarm keypair to sign gossipsub messages consistently
    pub fn new(local_peer_id: PeerId, keypair: &Keypair) -> Self {
        let message_id_fn = |message: &gossipsub::Message| {
            let mut hasher = DefaultHasher::new();
            message.data.hash(&mut hasher);
            MessageId::from(hasher.finish().to_string())
        };

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(2))
            .validation_mode(gossipsub::ValidationMode::Permissive)
            .message_id_fn(message_id_fn)
            .mesh_n_low(1)
            .mesh_n(2)
            .mesh_n_high(4)
            .mesh_outbound_min(1)
            .build()
            .expect("Valid gossipsub config");

        let gossipsub = Gossipsub::new(
            gossipsub::MessageAuthenticity::Signed(keypair.clone()),
            gossipsub_config,
        )
        .expect("Gossipsub created");

        let mdns = Mdns::new(MdnsConfig::default(), local_peer_id)
            .expect("mDNS created");

        AustroBehaviour { gossipsub, mdns }
    }
}
