// --- Existing Imports ---
// use alloy_primitives::{Address, U256};
use anyhow::{Context, Result};
use dotenv::dotenv;
use risc0_zkvm::{default_prover, ExecutorEnv};
use serde::{Deserialize, Serialize};
use std::env;
// --- Alloy Imports ---
// Keep sol macro and potentially primitives if used elsewhere directly
use alloy::sol;
use alloy::sol_types::SolCall; // Needed for call struct SIGNATURE if logging

// --- Risc0 Steel Imports ---
use risc0_steel::{
    alloy::providers::ProviderBuilder, // Steel uses alloy providers internally
    alloy::primitives::{Address, U256},
    ethereum::{EthEvmEnv, ETH_MAINNET_CHAIN_SPEC}, // Choose appropriate chain spec
    host::BlockNumberOrTag,
    Contract, // The main steel contract interaction type
};
use url::Url; // For parsing URLs

// --- Reqwest Alias ---
use reqwest::Client as SubgraphReqwestClient;

// Import guest ELF and Image ID
use top_n_holders_guest_methods::{PROVE_TOP_N_HOLDERS_ELF, PROVE_TOP_N_HOLDERS_ID};

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
        function balanceOf(address account) external view returns (uint256);
    }
);

// --- Main Host Logic ---
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing/logging if desired (like in the example)
    // tracing_subscriber::fmt()
    //     .with_env_filter(EnvFilter::from_default_env())
    //     .init();

    dotenv().ok();

    // --- Configuration ---
    let subgraph_url = env::var("SUBGRAPH_URL")
        .context("SUBGRAPH_URL must be set")?;
    let rpc_url_str = env::var("RPC_URL").context("RPC_URL must be set")?;
    // Steel examples often use a Beacon API URL for state verification via EIP-4788
    let beacon_api_url_str = env::var("BEACON_API_URL")
        .context("BEACON_API_URL must be set (needed for EthEvmEnv builder)")?;
    let erc20_address_str = env::var("ERC20_ADDRESS")
        .context("ERC20_ADDRESS must be set")?;
    let n_str = env::var("N_TOP_HOLDERS").context("N_TOP_HOLDERS must be set")?;
    let claimed_top_n_str = env::var("CLAIMED_TOP_N_ADDRS")
        .context("CLAIMED_TOP_N_ADDRS must be set (comma-separated list)")?;

    let rpc_url: Url = rpc_url_str.parse().context("Invalid RPC_URL")?;
    let beacon_api_url: Url = beacon_api_url_str.parse().context("Invalid BEACON_API_URL")?;
    let erc20_contract_address: Address = erc20_address_str.parse()?;
    let n: usize = n_str.parse()?;
    let claimed_top_n_addresses: Vec<Address> = claimed_top_n_str
        .split(',')
        .map(|s| s.trim().parse::<Address>())
        .collect::<Result<Vec<_>, _>>()?;

    println!("Configuration:");
    println!("  ERC20 Contract: {}", erc20_contract_address);
    println!("  Subgraph URL: {}", subgraph_url);
    println!("  RPC URL: {}", rpc_url);
    println!("  Beacon API URL: {}", beacon_api_url);
    println!("  N: {}", n);
    println!("  Claimed Top N: {:?}", claimed_top_n_addresses);

    // --- Fetch Data from Subgraph (Unchanged) ---
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
            erc20_address_str.to_lowercase(),
            PAGE_SIZE,
            skip
        );

        let res = subgraph_http_client
            .post(&subgraph_url)
            .json(&serde_json::json!({ "query": graphql_query_paginated }))
            .send()
            .await?
            .error_for_status()?;

        let response_body: SubgraphResponse = res.json().await?;

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


    // --- Fetch Total Supply from Blockchain (using risc0-steel) ---
    println!("\nFetching total supply from blockchain via risc0-steel...");

    // 1. Create Alloy provider (used internally by EthEvmEnv)
    // We don't need to store this provider directly usually.
    let provider = ProviderBuilder::new().on_http(rpc_url.clone()); // Clone rpc_url if needed later

    // 2. Create EthEvmEnv
    let mut env = EthEvmEnv::builder()
        .provider(provider) // Pass the provider builder result
        // Fetch state from the latest block for totalSupply
        .block_number_or_tag(BlockNumberOrTag::Latest)
        .beacon_api(beacon_api_url) // Provide Beacon API URL
        .build()
        .await
        .context("Failed to build EthEvmEnv")?;

    // 3. Set the chain specification (IMPORTANT!)
    // Choose the correct spec for your network (e.g., Mainnet, Sepolia, Polygon, etc.)
    // Find available specs in risc0_steel::ethereum::chain_spec or define your own.
    env = env.with_chain_spec(&ETH_MAINNET_CHAIN_SPEC); // EXAMPLE: Using Sepolia spec

    // 4. Preflight the contract interaction using Contract::preflight
    // This prepares the environment for the call if needed by the guest later,
    // and allows us to make the call on the host now.
    let mut contract = Contract::preflight(erc20_contract_address, &mut env);

    // 5. Prepare the call data structure (generated by alloy::sol!)
    let call = IERC20::totalSupplyCall {};

    // 6. Execute the call using the contract helper and the call struct
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

    // 7. Extract the return value
    let onchain_total_supply: U256 = result._0; // Access the first return value

    println!("  On-chain Total Supply: {}", onchain_total_supply);


    // --- Decide which total supply to trust (Logic Unchanged) ---
    // It's CRITICAL that the value passed to the guest matches the sum check logic.
    // If the Subgraph is trusted to be up-to-date, use its value.
    // If on-chain is the source of truth, use that. Let's use on-chain here.
    let total_supply_for_guest = onchain_total_supply; // Directly use the alloy U256

    // --- Prepare Guest Input (Unchanged) ---
    let guest_input = GuestInput {
        all_holders: subgraph_holders,
        claimed_top_n_addresses: claimed_top_n_addresses.clone(),
        n,
        expected_total_supply: total_supply_for_guest,
    };

    // --- Execute and Prove (Unchanged) ---
    println!("\nExecuting and proving with Risk Zero zkVM...");
    // NOTE: The `EthEvmEnv` created above is *not* automatically passed to the guest.
    // The guest input (`guest_input`) is what's explicitly passed via `.write()`.
    // If your *guest* needed the EVM state (input created via `env.into_input().await?`),
    // you would `.write()` that input to the guest environment here.
    // For this specific use case (just proving holder list matches), the guest doesn't
    // directly need the `EthEvmEnv` input, only the holder data and expected total supply.
    let exec_env = ExecutorEnv::builder()
        .write(&guest_input)? // Pass application-specific input to the guest
        .build()?;

    let prover = default_prover();
    let prove_info = prover.prove(exec_env, PROVE_TOP_N_HOLDERS_ELF)?;
    let receipt = prove_info.receipt;
    println!("  Proof generated successfully!");

    // --- Verify the receipt (Unchanged) ---
    receipt.verify(PROVE_TOP_N_HOLDERS_ID)?;
    println!("  Receipt verified locally successfully!");

    // --- Extract and Print Results (Unchanged) ---
    let result: bool = receipt.journal.decode()?;
    println!("\nVerification Result (from ZK proof journal): {}", result);

    println!("\nData for On-Chain Verification:");
    println!("  Image ID: {:?}", PROVE_TOP_N_HOLDERS_ID);
    println!("  Journal: 0x{}", hex::encode(&receipt.journal.bytes));
    // Seal encoding depends on target verifier...

    if result {
        println!("\nConclusion: The provided list IS the correct Top {} holders.", n);
    } else {
        println!("\nConclusion: The provided list IS NOT the correct Top {} holders.", n);
    }

    Ok(())
}