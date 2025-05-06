// use risc0_zkvm::sha::Digest;
use alloy_primitives::{Address};
use serde::{Deserialize, Serialize};

// #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
// pub struct Outputs {
//     pub data: u32,
//     pub hash: Digest,
// }

// GuestInput: Data passed from the host to the ZKVM guest program.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GuestInput {
    pub required_addresses_desc: Vec<Address>, // The required addresses fetched from subgraph (DESC).
    pub n: usize,                     // The 'N' for Top-N.
    pub erc20_contract_address: Address,              // ERC20 token contract for balance checks.
    pub chain_spec_name: String,                      // Chain spec name for the guest.
}

// GuestOutput: Data returned from the ZKVM guest program via the journal.
// This definition must match the one used in the guest program.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GuestOutput {
    pub verification_succeeded: bool,       // True if all guest-side checks passed.
    pub final_top_n_addresses: Vec<Address>, // The Top-N addresses determined by the guest.
}
