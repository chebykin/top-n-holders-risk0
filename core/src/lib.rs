use std::collections::BTreeMap;
use std::sync::LazyLock;
use alloy_primitives::Address;
use serde::{Deserialize, Serialize};
use risc0_steel::config::{ChainSpec, ForkCondition};
use revm_primitives::hardfork::SpecId;

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

pub type GnosisChainSpec = ChainSpec<SpecId>;

/// The Gnosis Mainnet [ChainSpec].
pub static GNOSIS_MAINNET_CHAIN_SPEC: LazyLock<GnosisChainSpec> = LazyLock::new(|| ChainSpec {
    chain_id: 100, // Gnosis Chain Mainnet ID
    forks: BTreeMap::from([
        // Gnosis Chain Merge (Bellatrix+Paris)
        // Activated at block 24,424,400
        // Source: GnosisScan and community announcements
        (SpecId::MERGE, ForkCondition::Block(24_424_400)),

        // Gnosis Chain Shapella (Shanghai+Capella)
        // Activated at timestamp 1689076800 (July 11, 2023, 12:00:00 PM UTC)
        // Source: https://docs.gnosischain.com/about/history/upgrades#shapella-upgrade
        (SpecId::SHANGHAI, ForkCondition::Timestamp(1689076800)),

        // Gnosis Chain Dencun (Deneb+Cancun)
        // Activated at timestamp 1710160200 (March 11, 2024, 12:30:00 PM UTC)
        // Source: https://docs.gnosischain.com/about/history/upgrades#dencun-upgrade
        (SpecId::CANCUN, ForkCondition::Timestamp(1710160200)),

        // Prague/Pectra on Gnosis - Projected/TBD
        // Gnosis typically follows Ethereum mainnet hardforks.
        // This timestamp is a placeholder based on Ethereum Mainnet's projection
        // and should be updated when official Gnosis plans are announced.
        // Ethereum Mainnet Prague projection from your example: 1746612311
        (SpecId::PRAGUE, ForkCondition::Timestamp(1746612311)), // Placeholder, align with ETH Mainnet or update when Gnosis announces
    ]),
});
