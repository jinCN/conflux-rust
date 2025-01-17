use crate::{
    block_info::Round,
    term_state::{
        IN_QUEUE_LOCKED_VIEWS, OUT_QUEUE_LOCKED_VIEWS, ROUND_PER_TERM,
        TERM_ELECTED_SIZE, TERM_LIST_LEN, TERM_MAX_SIZE,
    },
};
use diem_crypto::_once_cell::sync::OnceCell;

#[derive(Clone, Debug)]
pub struct PosStateConfig {
    round_per_term: Round,
    term_max_size: usize,
    term_elected_size: usize,
    in_queue_locked_views: u64,
    out_queue_locked_views: u64,
}

pub trait PosStateConfigTrait {
    fn round_per_term(&self) -> Round;
    fn election_term_start_round(&self) -> Round;
    fn election_term_end_round(&self) -> Round;
    fn first_start_election_view(&self) -> u64;
    fn first_end_election_view(&self) -> u64;
    fn term_max_size(&self) -> usize;
    fn term_elected_size(&self) -> usize;
    fn in_queue_locked_views(&self) -> u64;
    fn out_queue_locked_views(&self) -> u64;
    fn force_retired_locked_views(&self) -> u64;
}

impl PosStateConfig {
    pub fn new(
        round_per_term: Round, term_max_size: usize, term_elected_size: usize,
        in_queue_locked_views: u64, out_queue_locked_views: u64,
    ) -> Self
    {
        Self {
            round_per_term,
            term_max_size,
            term_elected_size,
            in_queue_locked_views,
            out_queue_locked_views,
        }
    }
}

impl PosStateConfigTrait for OnceCell<PosStateConfig> {
    fn round_per_term(&self) -> Round { self.get().unwrap().round_per_term }

    /// A term `n` is open for election in the view range
    /// `(n * ROUND_PER_TERM - ELECTION_TERM_START_ROUND, n * ROUND_PER_TERM -
    /// ELECTION_TERM_END_ROUND]`
    fn election_term_start_round(&self) -> Round {
        self.round_per_term() / 2 * 3
    }

    fn election_term_end_round(&self) -> Round { self.round_per_term() / 2 }

    fn first_start_election_view(&self) -> u64 {
        TERM_LIST_LEN as u64 * self.round_per_term()
            - self.election_term_start_round()
    }

    fn first_end_election_view(&self) -> u64 {
        TERM_LIST_LEN as u64 * self.round_per_term()
            - self.election_term_end_round()
    }

    fn term_max_size(&self) -> usize { self.get().unwrap().term_max_size }

    fn term_elected_size(&self) -> usize {
        self.get().unwrap().term_elected_size
    }

    fn in_queue_locked_views(&self) -> u64 {
        self.get().unwrap().in_queue_locked_views
    }

    fn out_queue_locked_views(&self) -> u64 {
        self.get().unwrap().out_queue_locked_views
    }

    fn force_retired_locked_views(&self) -> u64 {
        self.out_queue_locked_views()
    }
}

pub static POS_STATE_CONFIG: OnceCell<PosStateConfig> = OnceCell::new();

impl Default for PosStateConfig {
    fn default() -> Self {
        Self {
            round_per_term: ROUND_PER_TERM,
            term_max_size: TERM_MAX_SIZE,
            term_elected_size: TERM_ELECTED_SIZE,
            in_queue_locked_views: IN_QUEUE_LOCKED_VIEWS,
            out_queue_locked_views: OUT_QUEUE_LOCKED_VIEWS,
        }
    }
}
