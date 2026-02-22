use axum::{
    extract::{Query, State},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tracing::{info, warn};

use crate::{
    arbitrage::{
        detector::detect_opportunities,
        strategy::Strategy,
    },
    config::Config,
    monad::{
        pools::{fetch_kuru_pool_snapshot, fetch_pool_snapshot, fetch_v3_pool_snapshot, PoolSnapshot},
        rpc::MonadProvider,
    },
};

pub struct AppState {
    pub provider: MonadProvider,
    pub nadfun_provider: MonadProvider,
    pub config: Config,
    pub http_client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
pub struct DexArbParams {
    pub wallet: Option<String>,
    #[serde(default)]
    pub strategy: Strategy,
}

/// Serializable pool info for the response.
#[derive(Debug, Serialize)]
pub struct PoolInfo {
    pub dex: String,
    pub pair: String,
    pub token_in: String,
    pub token_out: String,
    pub rate: f64,
    pub liquidity_usd: f64,
    pub fee_pct: f64,
    pub source_chain: String,
}

impl From<&PoolSnapshot> for PoolInfo {
    fn from(p: &PoolSnapshot) -> Self {
        PoolInfo {
            dex: p.dex.clone(),
            pair: p.pair.clone(),
            token_in: p.token_in.clone(),
            token_out: p.token_out.clone(),
            rate: p.rate,
            liquidity_usd: p.liquidity_usd,
            fee_pct: p.fee_pct,
            source_chain: p.source_chain.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DexArbResponse {
    pub wallet: Option<String>,
    pub strategy: String,
    /// All fetched pool edges — always populated for AI layer consumption
    pub pools: Vec<PoolInfo>,
    /// Classic 2-hop opportunities (DEX-DEX same-pair spread)
    pub opportunities: Vec<crate::arbitrage::detector::ArbOpportunity>,
    /// Plain-English summary for AI consumption
    pub market_context: String,
    pub block: u64,
    pub timestamp: String,
    pub network: String,
}

pub async fn dex_arb_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DexArbParams>,
) -> Json<Value> {
    let wallet = params.wallet.clone();
    let strategy = params.strategy;
    let cfg = &state.config;

    info!(
        wallet = ?wallet,
        strategy = strategy.as_str(),
        "DEX arb request"
    );

    // Get current block number
    let block = crate::monad::rpc::get_block_number(&state.provider)
        .await
        .unwrap_or(0);

    let mut pool_edges: Vec<PoolSnapshot> = Vec::new();

    // ── Kuru pools ─────────────────────────────────────────────────
    // Default: MON/USDC; then any extras from config
    let kuru_pairs: Vec<(&str, &str)> = std::iter::once(("MON/USDC", cfg.kuru_pool_mon_usdc.as_str()))
        .chain(
            cfg.extra_kuru_pools
                .iter()
                .map(|(label, addr)| (label.as_str(), addr.as_str())),
        )
        .collect();

    for (pair_label, pool_addr) in &kuru_pairs {
        match fetch_kuru_pool_snapshot(&state.provider, "Kuru", pair_label, pool_addr).await {
            Ok(edges) => {
                info!(dex = "Kuru", pair = pair_label, price = edges[0].spot_price, "Kuru CLOB snapshot");
                pool_edges.extend(edges);
            }
            Err(e) => warn!(dex = "Kuru", pair = pair_label, error = %e, "Kuru CLOB snapshot failed"),
        }
    }

    // ── Uniswap V3 pools ───────────────────────────────────────────
    let uv3_fee_pct = cfg.uniswap_v3_fee_tier as f64 / 1_000_000.0 * 100.0;

    // Default: MON/USDC
    match fetch_v3_pool_snapshot(
        &state.provider,
        "Uniswap V3",
        "MON/USDC",
        &cfg.uniswap_v3_factory,
        &cfg.wmon_address,
        &cfg.usdc_address,
        cfg.uniswap_v3_fee_tier,
        cfg.uniswap_v3_pool_mon_usdc.as_deref(),
        uv3_fee_pct,
    )
    .await
    {
        Ok(edges) => {
            info!(
                dex = "Uniswap V3",
                pair = "MON/USDC",
                price = edges[0].spot_price,
                liquidity = edges[0].liquidity_usd,
                "Pool snapshot"
            );
            pool_edges.extend(edges);
        }
        Err(e) => warn!(dex = "Uniswap V3", error = %e, "V3 slot0 read failed"),
    }

    // Extra UV3 pools (by pool address override; pair tokens must resolve from label)
    for (pair_label, pool_addr) in &cfg.extra_uv3_pools {
        // For extra pools we use fetch_pool_snapshot (V2 ABI) only if the pool supports it.
        // Extra UV3 pools with known addresses use the pool override path.
        // Since we can't determine token0/token1 from the label alone, we query via override.
        match fetch_v3_pool_snapshot(
            &state.provider,
            "Uniswap V3",
            pair_label,
            &cfg.uniswap_v3_factory,
            &cfg.wmon_address, // placeholder; pool override skips factory lookup
            &cfg.usdc_address,
            cfg.uniswap_v3_fee_tier,
            Some(pool_addr.as_str()),
            uv3_fee_pct,
        )
        .await
        {
            Ok(edges) => {
                info!(
                    dex = "Uniswap V3",
                    pair = pair_label,
                    price = edges[0].spot_price,
                    liquidity = edges[0].liquidity_usd,
                    "Extra pool snapshot"
                );
                pool_edges.extend(edges);
            }
            Err(e) => warn!(dex = "Uniswap V3", pair = pair_label, error = %e, "Extra V3 pool failed"),
        }
    }

    // ── PancakeSwap V2 (mainnet AMM, confirmed liquid) ─────────────
    // 0.25% fee tier for PancakeSwap V2
    if let Some(ref pair_addr) = cfg.pancake_v2_pool_mon_usdc {
        match fetch_pool_snapshot(&state.provider, "PancakeSwap V2", "MON/USDC", pair_addr, 0.25).await {
            Ok(edges) => {
                info!(dex = "PancakeSwap V2", price = edges[0].spot_price, liquidity = edges[0].liquidity_usd, "V2 AMM snapshot");
                pool_edges.extend(edges);
            }
            Err(e) => warn!(dex = "PancakeSwap V2", error = %e, "V2 getReserves failed"),
        }
    }

    // ── Classic 2-hop detector ─────────────────────────────────────
    let all_opps = detect_opportunities(&pool_edges);
    let filtered_opps: Vec<_> = all_opps
        .into_iter()
        .filter(|opp| strategy.filter(opp))
        .collect();

    // ── Build serializable pool list ───────────────────────────────
    let pools: Vec<PoolInfo> = pool_edges.iter().map(PoolInfo::from).collect();

    let market_context = build_market_context(&pools, &filtered_opps);

    info!(
        pool_count = pools.len(),
        opp_count = filtered_opps.len(),
        "DEX arb response"
    );

    let response = DexArbResponse {
        wallet,
        strategy: strategy.as_str().to_string(),
        pools,
        opportunities: filtered_opps,
        market_context,
        block,
        timestamp: Utc::now().to_rfc3339(),
        network: "monad-testnet".to_string(),
    };

    Json(serde_json::to_value(response).unwrap_or_default())
}

fn build_market_context(pools: &[PoolInfo], opps: &[crate::arbitrage::detector::ArbOpportunity]) -> String {
    // Collect unique pairs
    let mut pair_prices: std::collections::HashMap<String, Vec<(String, f64)>> =
        std::collections::HashMap::new();

    for pool in pools {
        // Only forward edges (token_in is the base token)
        // We use pools where token_out == "USDC" to get USD price of token_in
        if pool.token_out == "USDC" {
            pair_prices
                .entry(pool.pair.clone())
                .or_default()
                .push((pool.dex.clone(), pool.rate));
        }
    }

    let mut lines: Vec<String> = Vec::new();

    // Per-pair price summary
    for (pair, dex_prices) in &pair_prices {
        let base = pair.split('/').next().unwrap_or(pair);
        let price_parts: Vec<String> = dex_prices
            .iter()
            .map(|(dex, price)| format!("${:.4} on {}", price, dex))
            .collect();
        lines.push(format!("{} trading at {}.", base, price_parts.join(", ")));
    }

    // Opportunity summary
    if opps.is_empty() {
        lines.push("No actionable DEX-DEX arbitrage detected across scanned pairs.".to_string());
    } else {
        let best = &opps[0];
        lines.push(format!(
            "Best spread: {:.2}% on {} (net after fees: {:.2}%). Buy on {}, sell on {}.",
            best.spread_pct, best.pair, best.net_profit_pct, best.dex_buy, best.dex_sell
        ));
        if opps.len() > 1 {
            lines.push(format!("{} total opportunities found.", opps.len()));
        }
    }

    // Pool count
    let unique_pairs: std::collections::HashSet<&str> =
        pools.iter().map(|p| p.pair.as_str()).collect();
    lines.push(format!(
        "[{} pools scanned across {} pair(s).]",
        pools.len(),
        unique_pairs.len()
    ));

    lines.join(" ")
}
