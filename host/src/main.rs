// --- Existing Imports ---
use anyhow::{Context, Result};
use risc0_zkvm::{default_prover, ExecutorEnv};
use serde::{Deserialize, Serialize};
use std::str::FromStr; // For parsing Address with clap
use std::fs; // For file system operations (cache)
use std::path::Path; // For path manipulation (cache)

// --- Clap Imports ---
use clap::Parser;

// --- Alloy Imports ---
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::address;
// Needed for call struct SIGNATURE if logging

// --- Risc0 Steel Imports ---
use risc0_steel::{
    alloy::primitives::{Address, U256}, // Steel re-exports alloy primitives
    ethereum::{EthEvmEnv, ETH_MAINNET_CHAIN_SPEC}, // Choose appropriate chain spec
    Contract, // The main steel contract interaction type
};
use url::Url; // For parsing URLs via clap

// --- Reqwest Alias ---
use reqwest::Client as SubgraphReqwestClient;
use tracing::{error, info};
// Import guest ELF and Image ID
use top_n_holders_guest_methods::{TOP_N_HOLDERS_GUEST_ELF, TOP_N_HOLDERS_GUEST_ID};

// --- Logging Imports ---
use tracing_subscriber::EnvFilter;
use top_n_holders_core::{GuestInput, GuestOutput};
// --- Struct Definitions ---

#[derive(Serialize, Deserialize, Debug, Clone)]
struct HolderData {
    address: Address,
    balance: U256,
}

// SubgraphHolderResponse: Structure to deserialize individual holder entries from Subgraph.
#[derive(Deserialize, Debug)]
struct SubgraphHolderResponse {
    // The 'id' field now holds the holder's address string
    id: String,
    balance: String,
}

// SubgraphResponse: Structure to deserialize the top-level Subgraph API response.
#[derive(Deserialize, Debug)]
struct SubgraphResponse {
    data: SubgraphData,
}

// SubgraphData: Structure to deserialize the 'data' part of the Subgraph response.
#[derive(Deserialize, Debug)]
struct SubgraphData {
    #[serde(rename = "tokenHolders")] // Match the GraphQL query alias or field name
    token_holders: Vec<SubgraphHolderResponse>,
}

// --- Alloy setup for Contract Calls (used by steel) ---
sol!(
    interface IERC20 {
        function balanceOf(address account) external view returns (uint256);
        function totalSupply() external view returns (uint256);
    }

    // https://github.com/mds1/multicall
    interface IMulticall3 {
        struct Call3 {
            address target;
            bool allowFailure;
            bytes callData;
        }

        function aggregate3(Call3[] calldata calls)
            external
            payable
            returns (Result[] memory returnData);

        struct Result {
            bool success;
            bytes returnData;
        }
    }
);

// --- Clap Argument Parsing ---

#[derive(Parser, Debug)]
#[command(author, version, about = "Prove Top-N ERC20 Token Holders using Subgraph and Risc0", long_about = None)]
struct Args {
    /// URL of the GraphQL Subgraph endpoint providing token holder data.
    #[arg(long, env = "SUBGRAPH_URL")]
    subgraph_url: String, // Keep as String, URL parsing might be too strict

    /// URL of the JSON-RPC endpoint for the Ethereum node (e.g., Infura, Alchemy).
    #[arg(long, env = "RPC_URL")]
    rpc_url: Url,

    /// Address of the ERC20 token contract to verify.
    #[arg(long, env = "ERC20_ADDRESS", value_parser = Address::from_str)]
    erc20_address: Address,

    /// The number 'N' for Top-N holders verification.
    #[arg(long, env = "N_TOP_HOLDERS", value_parser = clap::value_parser!(usize))]
    n_top_holders: usize,

    /// Optional: Chain specification name (e.g., mainnet, sepolia). Defaults to mainnet.
    /// See risc0_steel::ethereum::chain_spec for available specs.
    #[arg(long, env = "CHAIN_SPEC", default_value = "mainnet")]
    chain_spec: String,

    /// Optional: Use Multicall3 for fetching balances. Defaults to false (fetch individually).
    #[arg(long, env = "USE_MULTICALL3", default_value_t = false)]
    multicall3: bool,
}

// --- Main Host Logic ---
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing/logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env()) // Use RUST_LOG env var
        .init();

    // Parse command-line arguments
    let args = Args::parse();

    // --- Configuration (from Args) ---
    let erc20_contract_address = args.erc20_address;
    let n = args.n_top_holders;
    let rpc_url = args.rpc_url; // Already Url type
    let subgraph_url = args.subgraph_url; // String

    println!("Configuration:");
    println!("  ERC20 Contract: {}", erc20_contract_address);
    println!("  Subgraph URL: {}", subgraph_url);
    println!("  RPC URL: {}", rpc_url);
    println!("  Chain Spec: {}", args.chain_spec);
    println!("  N: {}", n);

    // --- Cache Configuration ---
    let cache_dir = Path::new("./tmp");
    // Changed cache file name to reflect it stores addresses only.
    let cache_file_path = cache_dir.join("holder_addresses.json");

    // --- Attempt to Load from Cache or Fetch Data from Subgraph ---
    // Stores addresses fetched from the Subgraph.
    let mut all_subgraph_holders: Vec<HolderData>;

    if cache_file_path.exists() {
        println!("\nCache found at {:?}. Loading holder addresses from cache...", cache_file_path);
        let cached_data = fs::read_to_string(&cache_file_path)
            .with_context(|| format!("Failed to read cache file: {:?}", cache_file_path))?;
        // Deserialize as Vec<Address>.
        all_subgraph_holders = serde_json::from_str(&cached_data)
            .with_context(|| format!("Failed to deserialize cached data from {:?}", cache_file_path))?;
        println!("  Loaded {} holder addresses from cache.", all_subgraph_holders.len());

    } else {
        println!("\nCache not found. Fetching holder addresses from Subgraph...");
        let subgraph_http_client = SubgraphReqwestClient::new();
        let mut fetched_holders_list: Vec<HolderData> = Vec::new(); // Temporary list for fetching
        // Use last_id for pagination instead of skip
        let mut last_id = String::from(""); // Start with empty string for the first query
        const PAGE_SIZE: usize = 1000;

        loop {
            // Updated GraphQL query to fetch only holder IDs (addresses)
            let graphql_query_paginated = format!(
                r#"{{
                  tokenHolders(
                    first: {},
                    orderBy: id, # Order by ID for consistent pagination
                    orderDirection: asc, # Ascending order for id_gt
                    where: {{ token: "{}", id_gt: "{}" }}
                  ) {{
                    id # This is the holder's address
                    balance
                  }}
                }}"#,
                PAGE_SIZE,
                // Subgraphs often expect lowercase addresses in IDs/filters
                format!("{:#x}", erc20_contract_address).to_lowercase(),
                last_id // Use the last fetched ID for the filter
            );

            let res = subgraph_http_client
                .post(&subgraph_url)
                .json(&serde_json::json!({ "query": graphql_query_paginated }))
                .send()
                .await
                .context("Failed to send request to Subgraph")?;

            let status = res.status();
            let body_text = res.text().await.context("Failed to read Subgraph response body")?;

            if !status.is_success() {
                anyhow::bail!(
                    "Subgraph request failed with status: {}. Response body: {}",
                    status,
                    body_text
                );
            }

            let response_body: SubgraphResponse = serde_json::from_str(&body_text)
                .with_context(|| format!(
                    "Failed to decode Subgraph JSON response. Status: {}. Body: {}",
                    status,
                    body_text
                ))?;

            let fetched_holders_page = response_body.data.token_holders;
            let fetched_count = fetched_holders_page.len();
            // Log fetched count without skip
            println!("  Fetched page with {} holder addresses (last_id='{}')", fetched_count, last_id);

            if fetched_count == 0 {
                // No more holders found
                if last_id.is_empty() { // Check if this was the *first* query
                    println!("  No holders found for this token in the subgraph.");
                } else {
                    println!("  Finished fetching all holder addresses.");
                }
                break;
            }

            // Process fetched holders and update last_id
            if let Some(last_holder) = fetched_holders_page.last() {
                last_id = last_holder.id.clone(); // Update last_id for the next query
            }

            for holder_response in fetched_holders_page {
                let holder_address = Address::from_str(&holder_response.id)
                    .with_context(|| format!("Failed to parse holder address from id: {}", holder_response.id))?;
                let holder_balance = U256::from_str_radix(&holder_response.balance, 10)
                    .with_context(|| format!("Failed to parse balance for {}", holder_response.id))?;

                fetched_holders_list.push(HolderData { // Add to temporary list
                    address: holder_address,
                    balance: holder_balance,
                });
            }

            // Break if the fetched count is less than the page size (last page)
            if fetched_count < PAGE_SIZE { break; }
        }
        println!("  Fetched total {} holders from Subgraph.", fetched_holders_list.len());

        // Assign fetched data to the main variable
        all_subgraph_holders = fetched_holders_list;

        // --- Write to Cache ---
        println!("\nWriting fetched holder addresses to cache: {:?}", cache_file_path);
        fs::create_dir_all(cache_dir)
            .with_context(|| format!("Failed to create cache directory: {:?}", cache_dir))?;
        // Serialize Vec<Address> for caching.
        let cache_data = serde_json::to_string_pretty(&all_subgraph_holders)
            .context("Failed to serialize holder addresses for caching")?;
        fs::write(&cache_file_path, cache_data)
            .with_context(|| format!("Failed to write cache file: {:?}", cache_file_path))?;
        println!("  Successfully wrote cache file.");
    }

    // Host no longer determines Top-N directly. Guest will do this.
    println!(
        "\nSubgraph fetch complete. {} holder addresses will be passed to the ZKVM guest.",
        all_subgraph_holders.len()
    );
    println!("The guest will fetch balances on-chain, sort, verify total supply, and determine the Top {} holders.", n);

    // --- Fetch Total Supply from Blockchain (using risc0-steel) ---
    println!("\nFetching total supply from blockchain via risc0-steel...");

    let mut env = EthEvmEnv::builder()
        .rpc(rpc_url.clone()) // Ensure rpc_url is correctly passed
        .build()
        .await
        .context("Failed to build EthEvmEnv from RPC")?;

    match args.chain_spec.to_lowercase().as_str() {
        "mainnet" => {
            env = env.with_chain_spec(&ETH_MAINNET_CHAIN_SPEC);
            println!("  Using ETH_MAINNET_CHAIN_SPEC");
        },
        "sepolia" => {
            anyhow::bail!("Sepolia chain spec currently commented out in code. Please uncomment/add necessary import.");
        },
        _ => anyhow::bail!("Unsupported chain specification: {}", args.chain_spec),
    }

    let mut contract = Contract::preflight(erc20_contract_address, &mut env);

    let call = IERC20::totalSupplyCall {};

    println!(
        "  Calling {} on {}...",
        IERC20::totalSupplyCall::SIGNATURE,
        erc20_contract_address
    );
    let result_supply = contract // Renamed to avoid conflict if 'result' is used later for journal
        .call_builder(&call)
        .call()
        .await
        .context("Failed to call totalSupply via EthEvmEnv")?;

    let onchain_total_supply: U256 = result_supply._0;

    println!("  On-chain Total Supply: {}", onchain_total_supply);

    // --- Prepare Input for ZKVM Guest ---
    // The host provides its initial claim for the top N addresses.
    // This is at least N addresses from the subgraph, sorted by balance.
    // But usually it requires more than N to ensure the guest can determine the top N.
    // The guest will verify this claim by fetching balances and ensuring descending order.

    // Sort holders by descending balance
    all_subgraph_holders
        .sort_by(|a, b| {
            b.balance
                .cmp(&a.balance) // Descending balance
                .then_with(|| a.address.cmp(&b.address)) // Ascending address (tie-breaker)
        });

    // TODO: determine the holders required for the proof. Usually should be more than N.
    let mut required_addresses_desc: Vec<Address> = Vec::new();
    let mut accumulated_balance: U256 = U256::ZERO;
    let mut last_holder_balance: U256 = U256::ZERO;
    let mut threshold_balance: Option<U256> = None;
    let mut i = 0;
    // for ex. total supply is 100.
    //
    // A has 45, cumulative 45
    // B has 25, cumulative 70
    // C has 14, cumulative 84
    // D has 6, cumulative 90
    // E has 6, cumulative 96
    // F has 2, cumulative 98
    for holder in all_subgraph_holders.iter() {
        accumulated_balance += holder.balance;
        last_holder_balance = holder.balance;
        i += 1;
        if i == n {
            threshold_balance = Some(holder.balance);
        }

        required_addresses_desc.push(holder.address);
        // 100 - 84 = 16; sr16 > lb14, false
        // 100 - 90 = 10; sr10 > lb6, false
        // 100 - 96 = 4; sr4 < lb6, true
        // sr4100 < lb1;
        if let Some(threshold) = threshold_balance {
            let remainder = onchain_total_supply - accumulated_balance;
            info!("#{} Holder: {} - Balance: {}, Threshold: {}, Remainder: {}", i, holder.address, holder.balance, threshold, remainder);
            info!("{} < {}", threshold, remainder);
            if threshold > remainder {
                break;
            }
        }
    }
    println!("Required top holders/Total holders: {} / {}", required_addresses_desc.len(), all_subgraph_holders.len());
    println!("Accumulated/Last holder balance: {} / {}", accumulated_balance, last_holder_balance);
    println!("Required holders ({}): {:?}", required_addresses_desc.len(), required_addresses_desc);

    println!("\nFetching balances for required addresses from blockchain via risc0-steel...");

    if args.multicall3 {
        println!("  Using Multicall3 to fetch balances...");
        // --- Multicall3 Setup ---
        // Address of the Multicall3 contract (same on most chains)
        // https://github.com/mds1/multicall
        const MULTICALL3_ADDRESS: Address = address!("0xcA11bde05977b3631167028862bE2a173976CA11");

        let mut multicall_contract = Contract::preflight(MULTICALL3_ADDRESS, &mut env);

        let calls: Vec<IMulticall3::Call3> = required_addresses_desc
            .iter()
            .map(|&addr| {
                let balance_of_call = IERC20::balanceOfCall { account: addr };
                IMulticall3::Call3 {
                    target: erc20_contract_address, // The ERC20 token contract
                    allowFailure: true, // Allow individual calls to fail
                    callData: balance_of_call.abi_encode().into(),
                }
            })
            .collect();

        let aggregate_call = IMulticall3::aggregate3Call { calls };

        println!("  Preparing to call aggregate3 on Multicall3 contract at {}", MULTICALL3_ADDRESS);
        let multicall_results = multicall_contract
            .call_builder(&aggregate_call)
            .call()
            .await
            .context("Failed to call aggregate3 on Multicall3 contract")?;

        println!("  Multicall3 aggregate3 call successful. Processing {} results...", multicall_results.returnData.len());

        for (i, result) in multicall_results.returnData.iter().enumerate() {
            let holder_address = required_addresses_desc[i]; // Assuming order is preserved
            if result.success {
                match IERC20::balanceOfCall::abi_decode_returns(&result.returnData, true) {
                    Ok(decoded_balance) => {
                        info!("  Successfully fetched balance for {}: {}", holder_address, decoded_balance._0);
                    }
                    Err(e) => {
                        error!("  Failed to decode balanceOf return data for {}: {:?}", holder_address, e);
                    }
                }
            } else {
                info!("  balanceOf call failed for address {} in multicall", holder_address);
            }
        }
    } else {
        println!("  Fetching balances individually (not using Multicall3)...");
        let mut individual_balances: Vec<(Address, U256)> = Vec::new(); // To store fetched balances if needed

        for (i, &holder_address) in required_addresses_desc.iter().enumerate() {
            println!("  Fetching balance for address {} ({}/{})", holder_address, i + 1, required_addresses_desc.len());
            let balance_of_call = IERC20::balanceOfCall { account: holder_address };
            let mut individual_contract_instance = Contract::preflight(erc20_contract_address, &mut env);

            match individual_contract_instance
                .call_builder(&balance_of_call)
                .call()
                .await
            {
                Ok(result_balance) => {
                    let balance: U256 = result_balance._0;
                    info!("  Successfully fetched balance for {}: {}", holder_address, balance);
                    individual_balances.push((holder_address, balance));
                    // As before, this is mostly for pre-warming the EVM state for the guest.
                }
                Err(e) => {
                    error!("  Failed to fetch balance for {}: {:?}", holder_address, e);
                    // Decide how to handle individual errors, e.g., push a zero balance or skip
                }
            }
        }
        println!("  Finished fetching balances individually for {} addresses.", required_addresses_desc.len());
    }

    let guest_input = GuestInput {
        required_addresses_desc,
        n,
        erc20_contract_address,
        chain_spec_name: args.chain_spec.clone(), // Pass chain spec name
    };

    let evm_input = env.into_input().await?;

    println!("\nExecuting and proving with Risk Zero zkVM...");
    let exec_env = ExecutorEnv::builder()
        .write(&evm_input)?
        .write(&guest_input)?
        .build()?;

    let prover = default_prover();
    println!("  Running the prover...");
    let prove_info = prover.prove(exec_env, TOP_N_HOLDERS_GUEST_ELF)?;
    let receipt = prove_info.receipt;
    println!("  Proof generated successfully!");

    receipt.verify(TOP_N_HOLDERS_GUEST_ID)?;
    println!("  Receipt verified locally successfully!");

    // Decode GuestOutput from the journal.
    let guest_output: GuestOutput = receipt.journal.decode()
        .context("Failed to decode GuestOutput from ZKVM journal")?;

    println!("\nVerification Result (from ZK proof journal):");
    println!("  Guest Verification Succeeded: {}", guest_output.verification_succeeded);
    println!("  Guest Determined Top {} Addresses: {:?}", n, guest_output.final_top_n_addresses);
    println!("  (Proof implies guest correctly fetched balances, sorted, checked total supply, and compared against host's claimed Top {} addresses)", n);

    println!("\nData for On-Chain Verification:");
    println!("  Image ID: {:?}", TOP_N_HOLDERS_GUEST_ID);
    println!("  Journal (Hex): 0x{}", hex::encode(&receipt.journal.bytes));

    if guest_output.verification_succeeded {
        println!("\nConclusion: The ZK proof confirms the guest correctly determined the Top {} holders, verified total supply, and that these match the host's initial claim.", n);
        println!("  The determined Top {} addresses by the guest are: {:?}", n, guest_output.final_top_n_addresses);
    } else {
        println!("\nConclusion: The ZK proof indicates a discrepancy or failure in guest execution.");
        println!("  This could be due to: total supply mismatch, or the guest's determined Top-N differs from the host's claimed Top-N, or other internal guest error.");
        if !guest_output.final_top_n_addresses.is_empty() {
             println!("  Guest's determined Top {} addresses (if available): {:?}", n, guest_output.final_top_n_addresses);
        } else {
            println!("  Guest did not determine/output Top-N addresses, or an earlier error occurred (e.g., balance fetch, total supply mismatch).");
        }
    }

    Ok(())
}