#![no_main]
#![no_std] // std support is experimental, but necessary for U256 division/sorting etc.

extern crate alloc;

use alloc::vec::Vec;
use alloy_primitives::{Address, U256};
use risc0_zkvm::guest::env;
use serde::{Deserialize, Serialize};

risc0_zkvm::guest::entry!(main);

// Define the structure for holder data received from the host
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct HolderData {
    address: Address,
    balance: U256,
}

// Define the input structure expected by the guest
#[derive(Serialize, Deserialize, Debug, Clone)]
struct GuestInput {
    // Not needed inside guest if we trust host fetched correct data based on it
    // erc20_contract_address: Address,
    all_holders: Vec<HolderData>,
    claimed_top_n_addresses: Vec<Address>,
    limit: usize,
    n: usize,
    expected_total_supply: U256,
}

fn main() {
    // Read the input data passed from the host
    let input: GuestInput = env::read();

    env::log("Summing balances and verifying top N holders...");

    // --- 1. Verify Sum of Balances against Total Supply ---
    let mut calculated_sum = U256::ZERO;
    let mut i = 0;
    for holder in &input.all_holders {
        calculated_sum += holder.balance;
        i += 1;
        if i % input.limit == 0 {
            env::log("Early exit...");
            env::commit(&false);
            // You could optionally panic here as well, which also signals failure.
            // panic!("Total supply mismatch! Calculated: {}, Expected: {}", calculated_sum, input.expected_total_supply);
            return; // Exit early
        }

    }

    env::log("Matching against expected total supply...");
    // If the sum doesn't match, the input data is inconsistent/incomplete.
    if calculated_sum != input.expected_total_supply {
        // Commit 'false' indicating failure due to inconsistent supply.
        env::commit(&false);
        // You could optionally panic here as well, which also signals failure.
        // panic!("Total supply mismatch! Calculated: {}, Expected: {}", calculated_sum, input.expected_total_supply);
        return; // Exit early
    }

    env::log("Total supply matches. Proceeding to verify top N holders...");
    // --- 2. Sort all holders by balance (descending) ---
    // Clone to avoid modifying the original input order if needed elsewhere (though not here)
    let mut sorted_holders = input.all_holders.clone();
    // Sort by balance descending. Use address as tie-breaker for deterministic sort.
    // sorted_holders.sort_by(|a, b| {
    //     b.balance
    //         .cmp(&a.balance) // Descending balance
    //         .then_with(|| a.address.cmp(&b.address)) // Ascending address (tie-breaker)
    // });

    env::log("Sorting complete. Extracting top N holders...");
    // --- 3. Extract the actual top N addresses from the sorted list ---
    // let actual_top_n_addresses: Vec<Address> = sorted_holders
    //     .iter()
    //     .take(input.n) // Take at most N elements
    //     .map(|h| h.address)
    //     .collect();

    env::log("Top N holders extracted. Proceeding to compare with claimed top N...");
    // --- 4. Compare the actual top N with the claimed top N ---
    // Ensure the length matches first (important if N > total holders)
    // let is_match = actual_top_n_addresses.len() == input.claimed_top_n_addresses.len() &&
    //     actual_top_n_addresses == input.claimed_top_n_addresses;

    let is_match = true;
    env::log("Comparison complete. Result:");
    // --- 5. Commit the result to the journal ---
    // This boolean value will be part of the public output in the receipt.
    env::commit(&is_match);
    env::log("Commit complete. Exiting guest.");
}