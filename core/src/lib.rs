use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

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

//// The Gnosis Mainnet [ChainSpec] (Block numbers could be incorrect).
// pub static GNOSIS_MAINNET_CHAIN_SPEC: LazyLock<ChainSpec> = LazyLock::new(|| ChainSpec {
//     chain_id: 100,
//     forks: BTreeMap::from([
//         (SpecId::MERGE, ForkCondition::Block(25139031)),    // Approx. Dec 8, 2022
//         (SpecId::SHANGHAI, ForkCondition::Timestamp(1689076787)), // July 11, 2023
//         (SpecId::CANCUN, ForkCondition::Timestamp(1710167400)), // March 11, 2024, 14:30 UTC
//     ]),
// });
