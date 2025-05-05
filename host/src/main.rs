// --- Existing Imports ---
use anyhow::{Context, Result};
use risc0_zkvm::{default_prover, ExecutorEnv};
use serde::{Deserialize, Serialize};
use std::str::FromStr; // For parsing Address with clap

// --- Clap Imports ---
use clap::Parser;

// --- Alloy Imports ---
use alloy::sol;
use alloy::sol_types::SolCall; // Needed for call struct SIGNATURE if logging

// --- Risc0 Steel Imports ---
use risc0_steel::{
    alloy::primitives::{Address, U256}, // Steel re-exports alloy primitives
    ethereum::{EthEvmEnv, ETH_MAINNET_CHAIN_SPEC}, // Choose appropriate chain spec
    Contract, // The main steel contract interaction type
};
use url::Url; // For parsing URLs via clap

// --- Reqwest Alias ---
use reqwest::Client as SubgraphReqwestClient;

// Import guest ELF and Image ID
use top_n_holders_guest_methods::{PROVE_TOP_N_HOLDERS_ELF, PROVE_TOP_N_HOLDERS_ID};

// --- Logging Imports ---
use tracing_subscriber::EnvFilter;

// --- Structs (Unchanged) ---
#[derive(Serialize, Deserialize, Debug, Clone)]
struct HolderData {
    address: Address,
    balance: U256,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct GuestInput {
    all_holders: Vec<HolderData>,
    claimed_top_n_addresses: Vec<Address>,
    n: usize,
    expected_total_supply: U256,
}

// Updated struct to match the new query response for tokenHolders
#[derive(Deserialize, Debug)]
struct SubgraphHolderResponse {
    // The 'id' field now holds the holder's address string
    id: String,
    balance: String,
}

#[derive(Deserialize, Debug)]
struct SubgraphResponse {
    data: SubgraphData,
}

// Updated SubgraphData to contain tokenHolders directly
#[derive(Deserialize, Debug)]
struct SubgraphData {
    #[serde(rename = "tokenHolders")] // Match the GraphQL query alias or field name
    token_holders: Vec<SubgraphHolderResponse>,
}

// --- Alloy setup for Contract Calls (used by steel) ---
sol!(
    interface IERC20 {
        function totalSupply() external view returns (uint256);
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
    println!("  Chain Spec: {}", args.chain_spec); // Added chain spec info
    println!("  N: {}", n);

    // --- Fetch Data from Subgraph ---
    println!("\nFetching data from Subgraph...");
    let subgraph_http_client = SubgraphReqwestClient::new();
    let mut subgraph_holders: Vec<HolderData> = Vec::new();
    // Use last_id for pagination instead of skip
    let mut last_id = String::from(""); // Start with empty string for the first query
    const PAGE_SIZE: usize = 1000;

    loop {
        // Updated GraphQL query to use id_gt for pagination
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

        let fetched_holders = response_body.data.token_holders;
        let fetched_count = fetched_holders.len();
        // Log fetched count without skip
        println!("  Fetched page with {} holders (last_id='{}')", fetched_count, last_id);

        if fetched_count == 0 {
            // No more holders found
            if last_id.is_empty() { // Check if this was the *first* query
                println!("  No holders found for this token in the subgraph.");
            } else {
                println!("  Finished fetching all holders.");
            }
            break;
        }

        // Process fetched holders and update last_id
        if let Some(last_holder) = fetched_holders.last() {
            last_id = last_holder.id.clone(); // Update last_id for the next query
        }

        for holder_response in fetched_holders {
            let holder_address = Address::from_str(&holder_response.id)
                .with_context(|| format!("Failed to parse holder address from id: {}", holder_response.id))?;
            let holder_balance = U256::from_str_radix(&holder_response.balance, 10)
                .with_context(|| format!("Failed to parse balance for {}", holder_response.id))?;

            subgraph_holders.push(HolderData {
                address: holder_address,
                balance: holder_balance,
            });
        }

        // Break if the fetched count is less than the page size (last page)
        if fetched_count < PAGE_SIZE { break; }
    }
    println!("  Fetched total {} holders from Subgraph.", subgraph_holders.len());

    // --- Determine Top N Addresses from Fetched Data ---
    println!("\nDetermining Top {} addresses from Subgraph data (sorting by balance)...", n);
    let mut sorted_subgraph_holders = subgraph_holders.clone();
    sorted_subgraph_holders.sort_by(|a, b| {
        b.balance
            .cmp(&a.balance)
            .then_with(|| a.address.cmp(&b.address))
    });

    let determined_top_n_addresses: Vec<Address> = sorted_subgraph_holders
        .iter()
        .take(n.min(sorted_subgraph_holders.len()))
        .map(|h| h.address)
        .collect();

    println!("  Determined Top {} Addresses: {:?}", n, determined_top_n_addresses);
    if determined_top_n_addresses.len() < n && subgraph_holders.len() >= n {
         eprintln!("Warning: Less than N addresses determined ({}) even though enough holders exist ({}). This might indicate duplicate addresses or sorting issues.", determined_top_n_addresses.len(), subgraph_holders.len());
    } else if determined_top_n_addresses.len() < n {
         eprintln!("Warning: Determined only {} addresses because total holders ({}) is less than N ({}).", determined_top_n_addresses.len(), subgraph_holders.len(), n);
    }

    // --- Fetch Total Supply from Blockchain (using risc0-steel) ---
    println!("\nFetching total supply from blockchain via risc0-steel...");

    let mut env = EthEvmEnv::builder()
        .rpc(rpc_url.clone())
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
    let result = contract
        .call_builder(&call)
        .call()
        .await
        .context("Failed to call totalSupply via EthEvmEnv")?;

    let onchain_total_supply: U256 = result._0;

    println!("  On-chain Total Supply: {}", onchain_total_supply);

    let total_supply_for_guest = onchain_total_supply;

    let guest_input = GuestInput {
        all_holders: subgraph_holders,
        claimed_top_n_addresses: determined_top_n_addresses.clone(),
        n,
        expected_total_supply: total_supply_for_guest,
    };

    println!("\nExecuting and proving with Risk Zero zkVM...");
    let exec_env = ExecutorEnv::builder()
        .write(&guest_input)?
        .build()?;

    let prover = default_prover();
    println!("  Running the prover...");
    let prove_info = prover.prove(exec_env, PROVE_TOP_N_HOLDERS_ELF)?;
    let receipt = prove_info.receipt;
    println!("  Proof generated successfully!");

    receipt.verify(PROVE_TOP_N_HOLDERS_ID)?;
    println!("  Receipt verified locally successfully!");

    let result: bool = receipt.journal.decode()?;
    println!("\nVerification Result (from ZK proof journal): {}", result);
    println!("  (Proves that the guest correctly identified the Top {} addresses based on the provided holder list and total supply)", n);

    println!("\nData for On-Chain Verification:");
    println!("  Image ID: {:?}", PROVE_TOP_N_HOLDERS_ID);
    println!("  Journal (Hex): 0x{}", hex::encode(&receipt.journal.bytes));

    if result {
        println!("\nConclusion: The ZK proof confirms the guest correctly determined the Top {} holders based on the provided data.", n);
        println!("  The determined Top {} addresses are: {:?}", n, determined_top_n_addresses);
    } else {
        println!("\nConclusion: The ZK proof indicates a discrepancy. The guest could NOT confirm the Top {} holders based on the provided data (e.g., total supply mismatch).", n);
    }

    Ok(())
}