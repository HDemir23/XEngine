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
    api::dex_arb::AppState,
    monad::{
        nadfun::{fetch_nadfun_market_tokens, NadFunMarketToken},
        pools::{fetch_kuru_pool_snapshot, fetch_pool_snapshot},
        rpc::get_block_number,
    },
};

#[derive(Debug, Deserialize)]
pub struct NadFunParams {
    /// Max tokens to return (default: config value)
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct NadFunMarketResponse {
    pub tokens: Vec<NadFunMarketToken>,
    /// Plain-English summary for AI consumption
    pub market_summary: String,
    pub mon_price_usdc: f64,
    pub token_count: usize,
    pub block: u64,
    pub timestamp: String,
    pub network: String,
}

pub async fn nadfun_market_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<NadFunParams>,
) -> Json<Value> {
    let cfg = &state.config;
    let count = params.limit.unwrap_or(cfg.nadfun_top_tokens_count);

    info!(limit = count, "nad.fun market request");

    // 1. Block number from testnet
    let block = get_block_number(&state.provider).await.unwrap_or(0);

    // 2. MON/USDC price — Kuru CLOB first, PancakeSwap V2 fallback
    let mon_price_usdc = 'price: {
        match fetch_kuru_pool_snapshot(&state.provider, "Kuru", "MON/USDC", &cfg.kuru_pool_mon_usdc).await {
            Ok(edges) if edges[0].rate > 0.0 => {
                let p = edges[0].rate;
                info!(mon_price_usdc = p, "MON price from Kuru");
                break 'price p;
            }
            Err(e) => warn!(error = %e, "Kuru MON price failed, trying PancakeSwap V2"),
            _ => warn!("Kuru returned zero price, trying PancakeSwap V2"),
        }
        if let Some(ref pair_addr) = cfg.pancake_v2_pool_mon_usdc {
            match fetch_pool_snapshot(&state.provider, "PancakeSwap V2", "MON/USDC", pair_addr, 0.25).await {
                Ok(edges) => {
                    let p = edges[0].rate;
                    info!(mon_price_usdc = p, "MON price from PancakeSwap V2");
                    break 'price p;
                }
                Err(e) => warn!(error = %e, "PancakeSwap V2 MON price also failed"),
            }
        }
        warn!("All MON price sources failed; defaulting to 0");
        0.0
    };

    // 3. Fetch + enrich nad.fun tokens via Lens (mainnet provider)
    let tokens = fetch_nadfun_market_tokens(
        &state.http_client,
        &state.nadfun_provider,
        &cfg.nadfun_lens,
        &cfg.nadfun_api_url,
        &cfg.nadfun_known_tokens,
        count,
        mon_price_usdc,
    )
    .await;

    let market_summary = build_market_summary(&tokens, mon_price_usdc);
    let token_count = tokens.len();

    info!(
        token_count = token_count,
        mon_price_usdc = mon_price_usdc,
        "nad.fun market response"
    );

    let response = NadFunMarketResponse {
        tokens,
        market_summary,
        mon_price_usdc,
        token_count,
        block,
        timestamp: Utc::now().to_rfc3339(),
        network: "monad-mainnet".to_string(),
    };

    Json(serde_json::to_value(response).unwrap_or_default())
}

fn build_market_summary(tokens: &[NadFunMarketToken], mon_price_usdc: f64) -> String {
    if tokens.is_empty() {
        return format!(
            "nad.fun market snapshot: no token data available. MON price: ${:.4} USDC.",
            mon_price_usdc
        );
    }

    let mut lines = Vec::new();

    lines.push(format!(
        "nad.fun market snapshot: {} tokens analyzed.",
        tokens.len()
    ));

    // Graduated tokens
    let graduated: Vec<&NadFunMarketToken> =
        tokens.iter().filter(|t| t.graduated).collect();
    if !graduated.is_empty() {
        let syms: Vec<&str> = graduated.iter().map(|t| t.symbol.as_str()).collect();
        lines.push(format!(
            "{} token(s) have graduated to the DEX: {}.",
            graduated.len(),
            syms.join(", ")
        ));
    }

    // Close-to-graduation tokens
    let near_grad: Vec<&NadFunMarketToken> = tokens
        .iter()
        .filter(|t| t.signal == "close_to_graduation")
        .collect();
    for t in &near_grad {
        lines.push(format!(
            "{} is close to graduation (bonding curve ~80%+ full).",
            t.symbol
        ));
    }

    // Trending tokens (top volume, not yet graduated)
    let trending: Vec<&NadFunMarketToken> = tokens
        .iter()
        .filter(|t| t.signal == "trending")
        .collect();
    if !trending.is_empty() {
        let syms: Vec<&str> = trending.iter().map(|t| t.symbol.as_str()).collect();
        lines.push(format!("Trending by volume: {}.", syms.join(", ")));
    }

    // Price range for active meme tokens (non-graduated)
    let active_prices: Vec<f64> = tokens
        .iter()
        .filter(|t| !t.graduated && t.price_in_usdc > 0.0)
        .map(|t| t.price_in_usdc)
        .collect();

    if active_prices.len() >= 2 {
        let min = active_prices
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min);
        let max = active_prices
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        lines.push(format!(
            "Active meme token price range: ${:.6}–${:.6} USDC.",
            min, max
        ));
    }

    // MON price context
    if mon_price_usdc > 0.0 {
        lines.push(format!("MON price: ${:.4} USDC.", mon_price_usdc));
    } else {
        lines.push("MON price: unavailable.".to_string());
    }

    lines.join(" ")
}
