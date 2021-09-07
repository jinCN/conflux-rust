use std::sync::Arc;

use once_cell::sync::OnceCell;

use cfx_types::H256;
use diem_config::{config::NodeConfig, keys::ConfigKey};
use diem_crypto::HashValue;
use diem_types::{
    contract_event::ContractEvent,
    epoch_state::EpochState,
    reward_distribution_event::RewardDistributionEvent,
    term_state::{DisputeEvent, NodeID, UnlockEvent},
    validator_config::{ConsensusPrivateKey, ConsensusVRFPrivateKey},
};
use primitives::pos::{NodeId, PosBlockId};
use storage_interface::{DBReaderForPoW, DbReader};

use crate::{
    pos::{
        consensus::ConsensusDB,
        pos::{start_pos_consensus, DiemHandle},
    },
    spec::genesis::GenesisPosState,
    sync::ProtocolConfiguration,
    ConsensusGraph,
};
use diemdb::DiemDB;
use network::NetworkService;
use std::{fs, io::Read};

pub type PosVerifier = PosHandler;

/// This includes the interfaces that the PoW consensus needs from the PoS
/// consensus.
///
/// We assume the PoS service will be always available after `initialize()`
/// returns, so all the other interfaces will panic if the PoS service is not
/// ready.
pub trait PosInterface: Send + Sync {
    /// Wait for initialization.
    fn initialize(&self) -> Result<(), String>;

    /// Get a PoS block by its ID.
    ///
    /// Return `None` if the block does not exist or is not committed.
    fn get_committed_block(&self, h: &PosBlockId) -> Option<PosBlock>;

    /// Return the latest committed PoS block ID.
    /// This will become the PoS reference of the mined PoW block.
    fn latest_block(&self) -> PosBlockId;

    fn get_events(
        &self, from: &PosBlockId, to: &PosBlockId,
    ) -> Vec<ContractEvent>;

    fn get_epoch_ending_blocks(
        &self, start_epoch: u64, end_epoch: u64,
    ) -> Vec<PosBlockId>;

    fn get_reward_event(&self, epoch: u64) -> Option<RewardDistributionEvent>;

    fn get_epoch_state(&self, block_id: &PosBlockId) -> EpochState;

    fn diem_db(&self) -> &Arc<DiemDB>;
}

#[allow(unused)]
pub struct PosBlock {
    hash: PosBlockId,
    epoch: u64,
    round: u64,
    pivot_decision: H256,
    version: u64,
    /* parent: PosBlockId,
     * author: NodeId,
     * voters: Vec<NodeId>, */
}

pub struct PosHandler {
    pos: OnceCell<Box<dyn PosInterface>>,
    // Keep all tokio Runtime so they will not be dropped directly.
    diem_handler: OnceCell<DiemHandle>,
    enable_height: u64,
    pub conf: PosConfiguration,
}

impl PosHandler {
    pub fn new(conf: PosConfiguration, enable_height: u64) -> Self {
        Self {
            pos: OnceCell::new(),
            diem_handler: OnceCell::new(),
            enable_height,
            conf,
        }
    }

    pub fn initialize(
        &self, network: Arc<NetworkService>, consensus: Arc<ConsensusGraph>,
    ) -> Result<(), String> {
        if self.pos.get().is_some() {
            bail!("Initializing already-initialized PosHandler!");
        }
        let initial_nodes = read_initial_nodes_from_file(
            self.conf.pos_initial_nodes_path.as_str(),
        )?;
        let diem_handler = start_pos_consensus(
            &self.conf.diem_conf,
            network.clone(),
            self.conf.protocol_conf.clone(),
            Some((
                self.conf.bls_key.public_key(),
                self.conf.vrf_key.public_key(),
            )),
            initial_nodes
                .initial_nodes
                .into_iter()
                .map(|node| {
                    (NodeID::new(node.bls_key, node.vrf_key), node.voting_power)
                })
                .collect(),
        );
        debug!("PoS initialized");
        let pos_connection = PosConnection::new(
            diem_handler.diem_db.clone(),
            diem_handler.consensus_db.clone(),
        );
        diem_handler.pow_handler.initialize(consensus);
        if self.pos.set(Box::new(pos_connection)).is_err()
            || self.diem_handler.set(diem_handler).is_err()
        {
            bail!("PoS initialized twice!");
        }
        Ok(())
    }

    pub fn config(&self) -> &PosConfiguration { &self.conf }

    fn pos(&self) -> &Box<dyn PosInterface> { self.pos.get().unwrap() }

    pub fn is_enabled_at_height(&self, height: u64) -> bool {
        height >= self.enable_height
    }

    pub fn is_committed(&self, h: &PosBlockId) -> bool {
        self.pos().get_committed_block(h).is_some()
    }

    /// Check if `me` is equal to or extends `preds` (parent and referees).
    ///
    /// Since committed PoS blocks form a chain, and no pos block should be
    /// skipped, we only need to check if the round of `me` is equal to or plus
    /// one compared with the predecessors' rounds.
    ///
    /// Return `false` if `me` or `preds` contains non-existent PoS blocks.
    pub fn verify_against_predecessors(
        &self, me: &PosBlockId, preds: &Vec<PosBlockId>,
    ) -> bool {
        let me_round = match self.pos().get_committed_block(me) {
            None => {
                warn!("No pos block for me={:?}", me);
                return false;
            }
            Some(b) => (b.epoch, b.round),
        };
        for p in preds {
            let p_round = match self.pos().get_committed_block(p) {
                None => {
                    warn!("No pos block for pred={:?}", p);
                    return false;
                }
                Some(b) => (b.epoch, b.round),
            };
            if me_round < p_round {
                warn!("Incorrect round: me={:?}, pred={:?}", me_round, p_round);
                return false;
            }
        }
        true
    }

    pub fn get_pivot_decision(&self, h: &PosBlockId) -> Option<H256> {
        self.pos().get_committed_block(h).map(|b| b.pivot_decision)
    }

    pub fn get_latest_pos_reference(&self) -> PosBlockId {
        self.pos().latest_block()
    }

    pub fn get_unlock_nodes(
        &self, h: &PosBlockId, parent_pos_ref: &PosBlockId,
    ) -> Vec<(NodeId, u64)> {
        let unlock_event_key = UnlockEvent::event_key();
        let mut unlock_nodes = Vec::new();
        for event in self.pos().get_events(parent_pos_ref, h) {
            if *event.key() == unlock_event_key {
                let unlock_event = UnlockEvent::from_bytes(event.event_data())
                    .expect("key checked");
                let node_id = H256::from_slice(unlock_event.node_id.as_ref());
                let votes = unlock_event.unlocked;
                unlock_nodes.push((node_id, votes));
            }
        }
        unlock_nodes
    }

    pub fn get_disputed_nodes(
        &self, h: &PosBlockId, parent_pos_ref: &PosBlockId,
    ) -> Vec<NodeId> {
        let dispute_event_key = DisputeEvent::event_key();
        let mut disputed_nodes = Vec::new();
        for event in self.pos().get_events(parent_pos_ref, h) {
            if *event.key() == dispute_event_key {
                let dispute_event =
                    DisputeEvent::from_bytes(event.event_data())
                        .expect("key checked");
                disputed_nodes
                    .push(H256::from_slice(dispute_event.node_id.as_ref()));
            }
        }
        disputed_nodes
    }

    pub fn get_reward_distribution_event(
        &self, h: &PosBlockId, parent_pos_ref: &PosBlockId,
    ) -> Option<Vec<RewardDistributionEvent>> {
        if h == parent_pos_ref {
            return None;
        }
        let me_block = self.pos().get_committed_block(h)?;
        let parent_block = self.pos().get_committed_block(parent_pos_ref)?;
        if me_block.epoch == parent_block.epoch {
            return None;
        }
        let mut events = Vec::new();
        for epoch in parent_block.epoch..me_block.epoch {
            events.push(self.pos().get_reward_event(epoch)?);
        }
        Some(events)
    }

    pub fn diem_db(&self) -> &Arc<DiemDB> { self.pos().diem_db() }
}

pub struct PosConnection {
    pos_storage: Arc<DiemDB>,
    consensus_db: Arc<ConsensusDB>,
}

impl PosConnection {
    pub fn new(
        pos_storage: Arc<DiemDB>, consensus_db: Arc<ConsensusDB>,
    ) -> Self {
        Self {
            pos_storage,
            consensus_db,
        }
    }
}

impl PosInterface for PosConnection {
    fn initialize(&self) -> Result<(), String> { Ok(()) }

    fn get_committed_block(&self, h: &PosBlockId) -> Option<PosBlock> {
        debug!("get_committed_block: {:?}", h);
        let block_hash = h256_to_diem_hash(h);
        let committed_block = self
            .pos_storage
            .get_committed_block_by_hash(&block_hash)
            .ok()?;

        /*
        let parent;
        let author;
        if *h == PosBlockId::default() {
            // genesis has no block, and its parent/author will not be used.
            parent = PosBlockId::default();
            author = NodeId::default();
        } else {
            let block = self
                .pos_consensus_db
                .get_ledger_block(&block_hash)
                .map_err(|e| {
                    warn!("get_committed_block: err={:?}", e);
                    e
                })
                .ok()??;
            debug_assert_eq!(block.id(), block_hash);
            parent = diem_hash_to_h256(&block.parent_id());
            // NIL block has no author.
            author = H256::from_slice(block.author().unwrap_or(Default::default()).as_ref());
        }
         */
        debug!("pos_handler gets committed_block={:?}", committed_block);
        Some(PosBlock {
            hash: *h,
            epoch: committed_block.epoch,
            round: committed_block.round,
            pivot_decision: committed_block.pivot_decision.block_hash,
            /* parent,
             * author,
             * voters: ledger_info
             *     .signatures()
             *     .keys()
             *     .map(|author| H256::from_slice(author.as_ref()))
             *     .collect(), */
            version: committed_block.version,
        })
    }

    fn latest_block(&self) -> PosBlockId {
        diem_hash_to_h256(
            &self
                .pos_storage
                .get_latest_ledger_info_option()
                .expect("Initialized")
                .ledger_info()
                .consensus_block_id(),
        )
    }

    fn get_events(
        &self, from: &PosBlockId, to: &PosBlockId,
    ) -> Vec<ContractEvent> {
        let start_version = self
            .pos_storage
            .get_committed_block_by_hash(&h256_to_diem_hash(from))
            .expect("err reading ledger info for from")
            .version;
        let end_version = self
            .pos_storage
            .get_committed_block_by_hash(&h256_to_diem_hash(to))
            .expect("err reading ledger info for to")
            .version;
        self.pos_storage
            .get_events_by_version(start_version, end_version)
            .expect("err reading events")
    }

    fn get_epoch_ending_blocks(
        &self, start_epoch: u64, end_epoch: u64,
    ) -> Vec<PosBlockId> {
        self.pos_storage
            .get_epoch_ending_blocks(start_epoch, end_epoch)
            .expect("err reading epoch ending blocks")
            .into_iter()
            .map(|h| diem_hash_to_h256(&h))
            .collect()
    }

    fn get_reward_event(&self, epoch: u64) -> Option<RewardDistributionEvent> {
        self.pos_storage.get_reward_event(epoch).ok()
    }

    fn get_epoch_state(&self, block_id: &PosBlockId) -> EpochState {
        self.pos_storage
            .get_pos_state(&h256_to_diem_hash(block_id))
            .expect("parent of an ending_epoch block")
            .epoch_state()
            .clone()
    }

    fn diem_db(&self) -> &Arc<DiemDB> { &self.pos_storage }
}

pub struct PosConfiguration {
    pub bls_key: ConfigKey<ConsensusPrivateKey>,
    pub vrf_key: ConfigKey<ConsensusVRFPrivateKey>,
    pub diem_conf: NodeConfig,
    pub protocol_conf: ProtocolConfiguration,
    pub pos_initial_nodes_path: String,
}

fn diem_hash_to_h256(h: &HashValue) -> PosBlockId { H256::from(h.as_ref()) }
fn h256_to_diem_hash(h: &PosBlockId) -> HashValue {
    HashValue::new(h.to_fixed_bytes())
}

pub fn save_initial_nodes_to_file(path: &str, genesis_nodes: GenesisPosState) {
    fs::write(path, serde_json::to_string(&genesis_nodes).unwrap()).unwrap();
}

pub fn read_initial_nodes_from_file(
    path: &str,
) -> Result<GenesisPosState, String> {
    let mut file = fs::File::open(path)
        .map_err(|e| format!("failed to open initial nodes file: {:?}", e))?;

    let mut nodes_str = String::new();
    file.read_to_string(&mut nodes_str)
        .map_err(|e| format!("failed to read initial nodes file: {:?}", e))?;

    serde_json::from_str(nodes_str.as_str())
        .map_err(|e| format!("failed to parse initial nodes file: {:?}", e))
}
