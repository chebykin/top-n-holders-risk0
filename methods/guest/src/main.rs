#![no_main]
#![no_std]

extern crate alloc; // Make allocator crate available
use alloc::vec::Vec; // Import Vec

use alloy_primitives::{Address, U256};
use risc0_zkvm::guest::env;
use serde::{Deserialize, Serialize};

risc0_zkvm::guest::entry!(main); // Sets the entry point function

// Define the structure for holder data received from the host
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct HolderData {
    address: Address,
    balance: U256,
}

// Define the input structure expected by the guest
#[derive(Serialize, Deserialize, Debug, Clone)]
struct GuestInput {
    all_holders: Vec<HolderData>,
    claimed_top_n_addresses: Vec<Address>,
    n: usize,
    expected_total_supply: U256,
}

// This 'main' function is the entry point EXECUTED INSIDE THE ZKVM
fn main() {
    // Read the input data passed from the host
    let input: GuestInput = env::read();

    // --- 1. Verify Sum of Balances against Total Supply ---
    let mut calculated_sum = U256::ZERO;
    for holder in &input.all_holders {
        // Basic overflow check (though U256 handles large numbers)
        // In a real scenario, consider if malicious input could cause issues,
        // although U256 addition won't panic on overflow (it wraps).
        calculated_sum = calculated_sum.wrapping_add(holder.balance);
    }

    // If the sum doesn't match, the input data is inconsistent/incomplete.
    if calculated_sum != input.expected_total_supply {
        env::commit(&false); // Commit 'false' indicating failure
        return; // Exit early
    }

    // --- 2. Sort all holders by balance (descending) ---
    let mut sorted_holders = input.all_holders.clone();
    // Sort by balance descending. Use address as tie-breaker for deterministic sort.
    sorted_holders.sort_by(|a, b| {
        b.balance
            .cmp(&a.balance) // Descending balance
            .then_with(|| a.address.cmp(&b.address)) // Ascending address (tie-breaker)
    });

    // --- 3. Extract the actual top N addresses from the sorted list ---
    let actual_top_n_addresses: Vec<Address> = sorted_holders
        .iter()
        .take(input.n.min(sorted_holders.len())) // Take at most N or the total number of holders
        .map(|h| h.address)
        .collect();

    // --- 4. Compare the actual top N with the claimed top N ---
    // Ensure the length matches first (important if N > total holders)
    let is_match = actual_top_n_addresses.len() == input.claimed_top_n_addresses.len() &&
        actual_top_n_addresses == input.claimed_top_n_addresses;


    // --- 5. Commit the result to the journal ---
    // This boolean value will be part of the public output in the receipt.
    env::commit(&is_match);
}