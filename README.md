# xclaw-engine

Rust API server powering xClaw — a DeFi copilot on Monad with x402 micropayment gating.

Agents pay $0.10 USDC per request (via the x402 v2 protocol) to access live on-chain DeFi intelligence: DEX arbitrage scans and nad.fun meme token market data.

## Endpoints

| Method | Path | Cost | Description |
|--------|------|------|-------------|
| GET | `/health` | Free | Server health check |
| GET | `/arbitrage/dex` | $0.10 USDC | DEX arbitrage opportunities across Kuru, Uniswap V3, PancakeSwap V2 |
| GET | `/market/nadfun` | $0.10 USDC | nad.fun meme token market snapshot with graduation signals |

### `/arbitrage/dex` query params

| Param | Type | Description |
|-------|------|-------------|
| `wallet` | `string` (optional) | Wallet address for context |
| `strategy` | `string` (optional) | Filter strategy (default: `all`) |

### `/market/nadfun` query params

| Param | Type | Description |
|-------|------|-------------|
| `limit` | `number` (optional) | Max tokens to return (default: `NAD_FUN_TOP_TOKENS`) |

## x402 Payment Flow

1. Client makes a request — no payment header
2. Server responds `402 Payment Required` with a `payment-required` header (Base64-encoded JSON of payment requirements)
3. Client signs a USDC transfer on Monad testnet (eip155:10143) and retries with `payment-signature` header
4. Server settles via facilitator at `https://x402-facilitator.molandak.org`
5. On success, the response payload is returned

Payment token: USDC (`0x534b2f3A21130d7a60830c2Df862319e593943A3`) on Monad testnet.

## Architecture

```
src/
├── main.rs              Entry point — Axum router + server bootstrap
├── config.rs            Config struct loaded from environment variables
├── x402.rs              x402 v2 middleware (gate, settle, 402 response builder)
├── api/
│   ├── health.rs        GET /health
│   ├── dex_arb.rs       GET /arbitrage/dex — pool fetching + arb detection
│   └── nadfun_market.rs GET /market/nadfun — nad.fun token enrichment
├── monad/
│   ├── rpc.rs           Alloy provider setup + block number helper
│   ├── pools.rs         On-chain pool snapshots (Kuru CLOB, Uniswap V3, PancakeSwap V2)
│   └── nadfun.rs        nad.fun Lens contract queries + API enrichment
└── arbitrage/
    ├── detector.rs      2-hop DEX-DEX spread detection
    ├── graph.rs         Pool graph representation
    ├── slicer.rs        Pool data slicing utilities
    └── strategy.rs      Strategy filter enum
```

## Configuration

All config is loaded from environment variables. Defaults point to Monad mainnet for DEX data and Monad testnet for x402 settlement.

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `8080` | HTTP listen port |
| `MONAD_RPC_URL` | `https://rpc.monad.xyz` | Monad RPC for DEX data |
| `NAD_FUN_RPC_URL` | `https://rpc.monad.xyz` | Monad RPC for nad.fun |
| `NAD_FUN_API_URL` | `https://api.nad.fun/tokens` | nad.fun REST API |
| `NAD_FUN_LENS` | `0x7e78A8DE94f21804F7a17F4E8BF9EC2c872187ea` | nad.fun Lens contract |
| `NAD_FUN_TOP_TOKENS` | `10` | Default tokens returned by `/market/nadfun` |
| `KURU_POOL_MON_USDC` | `0x065C9d28E428A0db40191a54d33d5b7c71a9C394` | Kuru MON/USDC pool |
| `KURU_FEE_PCT` | `0.1` | Kuru fee percent |
| `PANCAKE_V2_POOL_MON_USDC` | `0x27aa322b3f8ba9d0041df99c33fe4f3cc135e054` | PancakeSwap V2 pool |
| `UNISWAP_V3_FACTORY` | `0x204faca1764b154221e35c0d20abb3c525710498` | Uniswap V3 factory |
| `UNISWAP_V3_FEE_TIER` | `3000` | Uniswap V3 fee tier (in bps * 100) |
| `WMON_ADDRESS` | `0x3bd359C1119dA7Da1D913D1C4D2B7c461115433A` | WMON token |
| `USDC_ADDRESS` | `0x754704Bc059F8C67012fEd69BC8A327a5aafb603` | USDC token (mainnet) |
| `PAY_TO_ADDRESS` | `0x000...000` | Address to receive x402 payments |
| `X402_ENABLED` | `true` | Set to `false` to bypass payment gating |
| `EXTRA_KURU_POOLS` | — | Extra Kuru pools: `LABEL:ADDRESS,...` |
| `EXTRA_UV3_POOLS` | — | Extra Uniswap V3 pools: `LABEL:ADDRESS,...` |
| `NADFUN_KNOWN_TOKENS` | — | Known nad.fun tokens: `LABEL:ADDRESS,...` |
| `RUST_LOG` | `info` | Log level filter |

## Running

```bash
# Development
cargo run

# With custom config
PORT=3000 PAY_TO_ADDRESS=0xYourAddress X402_ENABLED=true cargo run

# Release build
cargo build --release
./target/release/xclaw-engine
```

## Key Dependencies

- **axum** — async HTTP framework
- **alloy** — Ethereum/Monad RPC client
- **tokio** — async runtime
- **serde / serde_json** — serialization
- **reqwest** — HTTP client (facilitator calls, nad.fun API)
- **tower-http** — CORS + tracing middleware
- **base64** — x402 payment header encoding

## Network Details

| Network | Chain ID | CAIP-2 | Use |
|---------|----------|--------|-----|
| Monad Mainnet | — | — | DEX pool data, nad.fun |
| Monad Testnet | 10143 | `eip155:10143` | x402 USDC payment settlement |

x402 Facilitator: `https://x402-facilitator.molandak.org`
