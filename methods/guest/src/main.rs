#![no_main]
#![no_std] // std support is experimental, but necessary for U256 division/sorting etc.

extern crate alloc;

use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use top_n_holders_core::{GuestInput, GuestOutput};

use alloy_primitives::{Address, U256};
use alloy_sol_types::{sol};

// --- Risc0 Steel Imports ---

use risc0_steel::{
    ethereum::{
        ETH_MAINNET_CHAIN_SPEC,
    },
    Contract,
};
use risc0_steel::ethereum::EthEvmInput;
use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

// --- Alloy setup for Contract Calls (used by steel) ---
sol!(
    interface IERC20 {
        function balanceOf(address account) external view returns (uint256);
        function totalSupply() external view returns (uint256);
    }
);

// Define the structure for holder data, used internally after fetching balances
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct HolderData {
    address: Address,
    balance: U256,
}

fn main() {
    // Read the input data passed from the host
    let input: EthEvmInput = env::read();
    let guest_input: GuestInput = env::read();
    env::log("INFO: Guest program started. Input received.");

    // --- 0. Initialize Steel Environment ---

    env::log(&alloc::format!("INFO: Setting up EthEvmEnv for chain: {}", guest_input.chain_spec_name));
    let steel_evm_env = match guest_input.chain_spec_name.to_lowercase().as_str() {
        "mainnet" => input.into_env().with_chain_spec(&ETH_MAINNET_CHAIN_SPEC),
        _ => input.into_env(),
    };
    env::log("INFO: EthEvmEnv configured.");

    // --- 0.5. Verifying inputs ---
    env::log(&alloc::format!("INFO: Verifying input data..."));
    assert!(!guest_input.required_addresses_desc.is_empty(), "Holders list is empty");
    assert!(guest_input.n > 0, "N must be greater than 0");
    assert!(guest_input.n <= guest_input.required_addresses_desc.len(), "N exceeds number of holders");

    // --- 1. Fetch Balances for the required holders ---
    env::log(&alloc::format!("INFO: Fetching balances for {} holders...", guest_input.required_addresses_desc.len()));
    let erc20_contract = Contract::new(guest_input.erc20_contract_address, &steel_evm_env);

    // --- 1. Fetch total supply ---
    let call = IERC20::totalSupplyCall {};
    let total_supply_result = erc20_contract.call_builder(&call).call();
    env::log(&alloc::format!("INFO: Fetched total supply: {}", total_supply_result._0));

    // --- 1.5. Verify the total supply ---
    let mut latest_balance: Option<U256> = None;
    let mut top_holders_accumulated: U256 = U256::ZERO;
    let mut i = 0;

    // The holders array is sorted from the highest holder balance to the lowest one.
    let mut top_desc_holders: Vec<Address> = Vec::new();
    for holder_address in &guest_input.required_addresses_desc {
        let call = IERC20::balanceOfCall { account: *holder_address };
        let current_balance_result = erc20_contract.call_builder(&call).call();

        // Check if the balance is gte than the latest balance

        if let Some(prev_balance) = latest_balance {
            env::log(&alloc::format!("DEBUG: Current balance: {}, Latest balance: {}", current_balance_result._0, prev_balance));
            assert!(current_balance_result._0 <= prev_balance, "Balance is not lower than or equal to the latest balance");
        }
        latest_balance = Some(current_balance_result._0);
        top_holders_accumulated += current_balance_result._0;
        top_desc_holders.push(*holder_address);
        i += 1;

        // for ex. total supply is 100.
        //
        // A has 45, cumulative 45
        // B has 25, cumulative 70
        // C has 14, cumulative 84
        // D has 6, cumulative 90
        // E has 6, cumulative 96
        // F has 2, cumulative 98
        if i > guest_input.n {
            let supply_remainder: U256 = total_supply_result._0 - top_holders_accumulated;
            assert!(supply_remainder > U256::ZERO, "Top N holders exceed total supply");

            // 100 - 84 = 16; sr16 > lb14, false
            // 100 - 90 = 10; sr10 > lb6, false
            // 100 - 96 = 4; sr4 < lb6, true
            env::log(&alloc::format!("DEBUG: Supply remainder: {}, latest balance: {}", supply_remainder, latest_balance.unwrap()));
            if supply_remainder < latest_balance.unwrap() {
                break;
            }
        }
    }

    // --- 6. Commit the result to the journal ---
    let output = GuestOutput {
        verification_succeeded: true,
        final_top_n_addresses: top_desc_holders, // Commit the determined top N
    };
    env::commit(&output);
    env::log("INFO: Commit complete. Exiting guest.");
}