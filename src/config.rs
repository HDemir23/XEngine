use anyhow::Result;
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub monad_rpc_url: String,
    // Kuru CLOB
    pub kuru_pool_mon_usdc: String,
    pub kuru_fee_pct: f64,
    // PancakeSwap V2 AMM (mainnet fallback — confirmed working)
    pub pancake_v2_pool_mon_usdc: Option<String>,
    // Extra pool pairs (label, pool_address)
    pub extra_kuru_pools: Vec<(String, String)>,
    pub extra_uv3_pools: Vec<(String, String)>,
    // Uniswap V3
    pub uniswap_v3_factory: String,
    pub uniswap_v3_pool_mon_usdc: Option<String>,
    pub uniswap_v3_fee_tier: u32,
    pub wmon_address: String,
    pub usdc_address: String,
    // nad.fun (Monad mainnet)
    pub nadfun_rpc_url: String,
    pub nadfun_api_url: String,
    pub nadfun_lens: String,
    #[allow(dead_code)]
    pub nadfun_bonding_curve: String,
    pub nadfun_top_tokens_count: usize,
    pub nadfun_known_tokens: Vec<(String, String)>,
    // General
    pub port: u16,
    // x402 micropayments (stays on testnet via separate facilitator config)
    pub pay_to_address: String,
    pub x402_enabled: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // ── DEX data: Monad TESTNET ────────────────────────────────────────
        let monad_rpc_url = env::var("MONAD_RPC_URL")
            .unwrap_or_else(|_| "https://testnet-rpc.monad.xyz".to_string());

        // Kuru testnet MON-USDC CLOB (MONAD_DEX_REFERENCE §2.1)
        let kuru_pool_mon_usdc = env::var("KURU_POOL_MON_USDC")
            .unwrap_or_else(|_| "0xd8336cb07d4be511ccaf06b799851e1a80f98c71".to_string());

        let kuru_fee_pct = env::var("KURU_FEE_PCT")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.1);

        // PancakeSwap V2 WMON/USDC pair — confirmed deployed + liquid on mainnet
        let pancake_v2_pool_mon_usdc = Some(
            env::var("PANCAKE_V2_POOL_MON_USDC")
                .unwrap_or_else(|_| "0x27aa322b3f8ba9d0041df99c33fe4f3cc135e054".to_string()),
        );

        let extra_kuru_pools = parse_extra_pools(
            env::var("EXTRA_KURU_POOLS").unwrap_or_default().as_str(),
        );

        let extra_uv3_pools = parse_extra_pools(
            env::var("EXTRA_UV3_POOLS").unwrap_or_default().as_str(),
        );

        // UV3 testnet factory (MONAD_DEX_REFERENCE §1.3)
        let uniswap_v3_factory = env::var("UNISWAP_V3_FACTORY")
            .unwrap_or_else(|_| "0x961235a9020b05c44df1026d956d1f4d78014276".to_string());

        let uniswap_v3_pool_mon_usdc = env::var("UNISWAP_V3_POOL_MON_USDC").ok();

        let uniswap_v3_fee_tier = env::var("UNISWAP_V3_FEE_TIER")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(3000);

        // Mainnet token addresses
        let wmon_address = env::var("WMON_ADDRESS")
            .unwrap_or_else(|_| "0x3bd359C1119dA7Da1D913D1C4D2B7c461115433A".to_string());

        let usdc_address = env::var("USDC_ADDRESS")
            .unwrap_or_else(|_| "0x754704Bc059F8C67012fEd69BC8A327a5aafb603".to_string());

        // ── nad.fun: also mainnet ──────────────────────────────────────────
        let nadfun_rpc_url = env::var("NAD_FUN_RPC_URL")
            .unwrap_or_else(|_| "https://rpc.monad.xyz".to_string());

        let nadfun_api_url = env::var("NAD_FUN_API_URL")
            .unwrap_or_else(|_| "https://api.nad.fun/tokens".to_string());

        let nadfun_lens = env::var("NAD_FUN_LENS")
            .unwrap_or_else(|_| "0x7e78A8DE94f21804F7a17F4E8BF9EC2c872187ea".to_string());

        let nadfun_bonding_curve = env::var("NAD_FUN_BONDING_CURVE")
            .unwrap_or_else(|_| "0xA7283d07812a02AFB7C09B60f8896bCEA3F90aCE".to_string());

        let nadfun_top_tokens_count = env::var("NAD_FUN_TOP_TOKENS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(10);

        let nadfun_known_tokens = parse_extra_pools(
            env::var("NADFUN_KNOWN_TOKENS").unwrap_or_default().as_str(),
        );

        // ── General ───────────────────────────────────────────────────────
        let port = env::var("PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(8080);

        let pay_to_address = env::var("PAY_TO_ADDRESS")
            .unwrap_or_else(|_| "0x0000000000000000000000000000000000000000".to_string());

        let x402_enabled = env::var("X402_ENABLED")
            .map(|s| s.to_lowercase() != "false")
            .unwrap_or(true);

        Ok(Config {
            monad_rpc_url,
            kuru_pool_mon_usdc,
            kuru_fee_pct,
            pancake_v2_pool_mon_usdc,
            extra_kuru_pools,
            extra_uv3_pools,
            uniswap_v3_factory,
            uniswap_v3_pool_mon_usdc,
            uniswap_v3_fee_tier,
            wmon_address,
            usdc_address,
            nadfun_rpc_url,
            nadfun_api_url,
            nadfun_lens,
            nadfun_bonding_curve,
            nadfun_top_tokens_count,
            nadfun_known_tokens,
            port,
            pay_to_address,
            x402_enabled,
        })
    }
}

/// Parse "LABEL:ADDRESS,LABEL:ADDRESS,..." into Vec<(label, address)>.
fn parse_extra_pools(raw: &str) -> Vec<(String, String)> {
    raw.split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() { return None; }
            let colon = entry.rfind(':')?;
            let label = entry[..colon].trim().to_string();
            let addr = entry[colon + 1..].trim().to_string();
            if label.is_empty() || addr.is_empty() { return None; }
            Some((label, addr))
        })
        .collect()
}
