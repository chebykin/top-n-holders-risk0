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
    // host::BlockNumberOrTag, // Not explicitly needed with .rpc() setup
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

#[derive(Deserialize, Debug)]
struct SubgraphTokenHolder {
    #[serde(rename = "holderAddress")]
    holder_address: Address,
    balance: String,
}

#[derive(Deserialize, Debug)]
struct SubgraphTokenData {
    holders: Vec<SubgraphTokenHolder>,
    #[serde(rename = "totalSupply")]
    total_supply: String,
}

#[derive(Deserialize, Debug)]
struct SubgraphResponse {
    data: SubgraphData,
}

#[derive(Deserialize, Debug)]
struct SubgraphData {
    token: Option<SubgraphTokenData>,
}

// --- Alloy setup for Contract Calls (used by steel) ---
sol!(
    interface IERC20 {
        function totalSupply() external view returns (uint256);
        // balanceOf is not needed in the host for this specific logic, but keep if used elsewhere
        // function balanceOf(address account) external view returns (uint256);
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
    // NOTE: If adding more complex chain spec handling, adjust EthEvmEnv setup below.
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
    let mut subgraph_total_supply = U256::ZERO;
    let mut skip = 0;
    const PAGE_SIZE: usize = 1000;

    loop {
        let graphql_query_paginated = format!(
            r#"{{
              token(id: "{}") {{
                totalSupply # Fetch TS on first page only for efficiency
                holders(first: {}, skip: {}, orderBy: balance, orderDirection: desc) {{
                  holderAddress
                  balance
                }}
              }}
            }}"#,
            // Subgraphs often expect lowercase addresses in IDs
            format!("{:#x}", erc20_contract_address).to_lowercase(),
            PAGE_SIZE,
            skip
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

        if let Some(token_data) = response_body.data.token {
            if skip == 0 {
                subgraph_total_supply = U256::from_str_radix(&token_data.total_supply, 10)
                    .context("Failed to parse total supply from subgraph")?;
            }
            let fetched_count = token_data.holders.len();
            println!("  Fetched page with {} holders (skip={})", fetched_count, skip);
            for holder in token_data.holders {
                subgraph_holders.push(HolderData {
                    address: holder.holder_address,
                    balance: U256::from_str_radix(&holder.balance, 10)
                        .context(format!("Failed to parse balance for {}", holder.holder_address))?,
                });
            }
            if fetched_count < PAGE_SIZE { break; }
            skip += PAGE_SIZE;
        } else {
            println!("  Token not found in subgraph or no holders.");
            break;
        }
    }
    println!("  Fetched total {} holders from Subgraph.", subgraph_holders.len());
    println!("  Subgraph Total Supply: {}", subgraph_total_supply);

    // --- Determine Top N Addresses from Fetched Data ---
    println!("\nDetermining Top {} addresses from Subgraph data...", n);
    // Sort holders by balance descending, address ascending as tie-breaker (mirroring guest logic)
    let mut sorted_subgraph_holders = subgraph_holders.clone(); // Clone to keep original order if needed
    sorted_subgraph_holders.sort_by(|a, b| {
        b.balance
            .cmp(&a.balance)
            .then_with(|| a.address.cmp(&b.address))
    });

    // Take the top N addresses
    let determined_top_n_addresses: Vec<Address> = sorted_subgraph_holders
        .iter()
        .take(n.min(sorted_subgraph_holders.len())) // Handle cases where n > total holders
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

    // 1. Create EthEvmEnv using the RPC URL (simpler setup)
    let mut env = EthEvmEnv::builder()
        .rpc(rpc_url.clone()) // Use the RPC URL directly
        // .block_number_or_tag(...) // Defaults to Latest, usually sufficient
        .build()
        .await
        .context("Failed to build EthEvmEnv from RPC")?;

    // 2. Set the chain specification (IMPORTANT!)
    //    Select based on argument or keep default (Mainnet in this case)
    //    Add more robust matching/error handling if supporting many chains.
    match args.chain_spec.to_lowercase().as_str() {
        "mainnet" => {
            env = env.with_chain_spec(&ETH_MAINNET_CHAIN_SPEC);
            println!("  Using ETH_MAINNET_CHAIN_SPEC");
        },
        "sepolia" => {
            // Ensure you have the correct import if needed:
            // use risc0_steel::ethereum::ETH_SEPOLIA_CHAIN_SPEC;
            // env = env.with_chain_spec(Ð_SEPOLIA_CHAIN_SPEC);
            // println!("  Using ETH_SEPOLIA_CHAIN_SPEC");
            anyhow::bail!("Sepolia chain spec currently commented out in code. Please uncomment/add necessary import.");
        },
        // Add other chains as needed (e.g., optimism, arbitrum, polygon)
        _ => anyhow::bail!("Unsupported chain specification: {}", args.chain_spec),
    }

    // 3. Preflight the contract interaction
    let mut contract = Contract::preflight(erc20_contract_address, &mut env);

    // 4. Prepare the call data structure
    let call = IERC20::totalSupplyCall {};

    // 5. Execute the call on the host
    println!(
        "  Calling {} on {}...",
        IERC20::totalSupplyCall::SIGNATURE, // Log the function signature
        erc20_contract_address
    );
    let result = contract
        .call_builder(&call) // Pass the call struct reference
        .call() // Execute the call on the host via the env
        .await
        .context("Failed to call totalSupply via EthEvmEnv")?;

    // 6. Extract the return value
    let onchain_total_supply: U256 = result._0; // Access the first return value

    println!("  On-chain Total Supply: {}", onchain_total_supply);

    // --- Decide which total supply to trust (Logic Unchanged) ---
    let total_supply_for_guest = onchain_total_supply; // Using on-chain value

    // --- Prepare Guest Input ---
    // Use the determined top N addresses for the 'claimed_top_n_addresses' field
    let guest_input = GuestInput {
        all_holders: subgraph_holders, // Pass the original (unsorted) list or the sorted one
        claimed_top_n_addresses: determined_top_n_addresses.clone(), // Use the list determined by the host
        n,
        expected_total_supply: total_supply_for_guest,
    };

    // --- Execute and Prove (Unchanged) ---
    println!("\nExecuting and proving with Risk Zero zkVM...");
    // The guest doesn't need the EVM state directly for this proof, only the holder data.
    let exec_env = ExecutorEnv::builder()
        .write(&guest_input)? // Pass application-specific input
        .build()?;

    let prover = default_prover();
    println!("  Running the prover...");
    let prove_info = prover.prove(exec_env, PROVE_TOP_N_HOLDERS_ELF)?;
    let receipt = prove_info.receipt;
    println!("  Proof generated successfully!");

    // --- Verify the receipt (Unchanged) ---
    receipt.verify(PROVE_TOP_N_HOLDERS_ID)?;
    println!("  Receipt verified locally successfully!");

    // --- Extract and Print Results ---
    let result: bool = receipt.journal.decode()?;
    // Adjust the final messages
    println!("\nVerification Result (from ZK proof journal): {}", result);
    println!("  (Proves that the guest correctly identified the Top {} addresses based on the provided holder list and total supply)", n);

    println!("\nData for On-Chain Verification:");
    println!("  Image ID: {:?}", PROVE_TOP_N_HOLDERS_ID);
    println!("  Journal (Hex): 0x{}", hex::encode(&receipt.journal.bytes));
    // Seal encoding depends on target verifier... consider Bonsai or direct seal output if needed

    if result {
        println!("\nConclusion: The ZK proof confirms the guest correctly determined the Top {} holders based on the provided data.", n);
        println!("  The determined Top {} addresses are: {:?}", n, determined_top_n_addresses);
    } else {
        println!("\nConclusion: The ZK proof indicates a discrepancy. The guest could NOT confirm the Top {} holders based on the provided data (e.g., total supply mismatch).", n);
        // It's less likely to fail here if the host determines the list, unless the total supply check fails in the guest.
    }

    Ok(())
}