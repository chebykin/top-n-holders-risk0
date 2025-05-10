default:
	RUST_LOG=info RISC0_DEV_MODE=0 RISC0_INFO=1 \
	time ./target/release/top-n-holders-host \
	--subgraph-url https://api.studio.thegraph.com/query/110782/torn-token-subgraph/version/latest \
	--rpc-url https://ethereum-rpc.publicnode.com \
	--erc20-address 0x77777feddddffc19ff86db637967013e6c6a116c \
	--n-top-holders 5

gnosis:
	RUST_LOG=info RISC0_DEV_MODE=0 RISC0_INFO=1 \
	time ./target/release/top-n-holders-host \
	--subgraph-url https://api.studio.thegraph.com/query/110782/safe-token-gnosis-chain/version/latest \
	--rpc-url https://rpc.gnosischain.com \
	--erc20-address 0x4d18815d14fe5c3304e87b3fa18318baa5c23820 \
	--chain-spec gnosis \
	--n-top-holders 2