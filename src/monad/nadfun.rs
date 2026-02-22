use alloy::primitives::{Address, U256};
use alloy::sol;
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::str::FromStr;
use tracing::{info, warn};

use super::{pools::PoolEdge, rpc::MonadProvider};

// nad.fun Lens interface for price quotes
sol! {
    #[allow(missing_docs)]
    #[sol(rpc)]
    interface ILens {
        function getAmountOut(address token, uint256 amountIn, bool isBuy)
            external view returns (address router, uint256 amountOut);
        function isGraduated(address token) external view returns (bool);
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct NadToken {
    pub address: String,
    pub symbol: String,
    pub graduated: bool,
}

/// Fetch top nad.fun tokens via REST API.
/// Gracefully returns empty vec if the API is unavailable or shape is unexpected.
#[allow(dead_code)]
pub async fn fetch_top_nadfun_tokens(
    client: &reqwest::Client,
    api_url: &str,
    count: usize,
) -> Vec<NadToken> {
    let url = format!("{}?sort=volume&limit={}", api_url, count);
    info!(url = %url, "Fetching nad.fun top tokens");

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "nad.fun API request failed");
            return vec![];
        }
    };

    let body: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "nad.fun API JSON parse failed");
            return vec![];
        }
    };

    // Log raw response shape for debugging (truncate to avoid log spam)
    let raw_str = serde_json::to_string(&body).unwrap_or_default();
    let preview = if raw_str.len() > 500 { &raw_str[..500] } else { &raw_str };
    info!(preview = %preview, "nad.fun API raw response (truncated)");

    // Try to parse as array of token objects
    if let Some(arr) = body.as_array() {
        let tokens = parse_nad_tokens(arr, count);
        info!(count = tokens.len(), "Fetched nad.fun tokens from API");
        return tokens;
    }

    // Some APIs nest under a known key
    for key in &["tokens", "data", "result", "items"] {
        if let Some(arr) = body.get(key).and_then(|v| v.as_array()) {
            let tokens = parse_nad_tokens(arr, count);
            info!(count = tokens.len(), key = key, "Fetched nad.fun tokens from nested key");
            return tokens;
        }
    }

    warn!("nad.fun API response shape unknown — no tokens extracted");
    vec![]
}

#[allow(dead_code)]
fn parse_nad_tokens(arr: &[Value], count: usize) -> Vec<NadToken> {
    let mut tokens = Vec::new();
    for item in arr.iter().take(count) {
        // Try common field names for address
        let address = item
            .get("address")
            .or_else(|| item.get("tokenAddress"))
            .or_else(|| item.get("contract"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if address.is_empty() {
            continue;
        }

        let symbol = item
            .get("symbol")
            .or_else(|| item.get("ticker"))
            .or_else(|| item.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN")
            .to_string();

        let graduated = item
            .get("graduated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        tokens.push(NadToken {
            address: address.to_string(),
            symbol,
            graduated,
        });
    }
    tokens
}

/// Fetch price edges for nad.fun tokens via the Lens contract.
/// Each token produces two PoolEdges: MON→TOKEN and TOKEN→MON.
#[allow(dead_code)]
pub async fn fetch_nadfun_edges(
    provider: &MonadProvider,
    lens_addr: &str,
    tokens: &[NadToken],
    _wmon_address: &str,
) -> Vec<PoolEdge> {
    let lens_address = match Address::from_str(lens_addr) {
        Ok(a) => a,
        Err(e) => {
            warn!(error = %e, addr = %lens_addr, "Invalid nad.fun Lens address");
            return vec![];
        }
    };

    let lens = ILens::new(lens_address, provider.clone());

    // Amount in: 1 MON = 1e18 (18 decimals)
    let one_mon = U256::from(1_000_000_000_000_000_000u128);

    let mut edges = Vec::new();

    for token in tokens {
        let token_addr = match Address::from_str(&token.address) {
            Ok(a) => a,
            Err(_) => {
                warn!(token = %token.symbol, "Invalid token address, skipping");
                continue;
            }
        };

        // Query MON → TOKEN rate (isBuy = true: buying token with MON)
        let buy_result = lens
            .getAmountOut(token_addr, one_mon, true)
            .call()
            .await;

        let amount_out_buy = match buy_result {
            Ok(r) => r.amountOut,
            Err(e) => {
                warn!(
                    token = %token.symbol,
                    error = %e,
                    "nad.fun Lens getAmountOut (buy) failed, skipping"
                );
                continue;
            }
        };

        // Convert to f64 rate: TOKEN per 1 MON
        // Assume token has 18 decimals (most meme tokens on nad.fun)
        let token_decimals = 18u32;
        let rate_mon_to_token =
            amount_out_buy.to::<u128>() as f64 / 10f64.powi(token_decimals as i32);

        if rate_mon_to_token <= 0.0 {
            warn!(token = %token.symbol, "Zero rate from nad.fun Lens (buy), skipping");
            continue;
        }

        // Query TOKEN → MON rate (isBuy = false: selling token for MON)
        let one_token = U256::from(10u128.pow(token_decimals));
        let sell_result = lens
            .getAmountOut(token_addr, one_token, false)
            .call()
            .await;

        let rate_token_to_mon = match sell_result {
            Ok(r) => r.amountOut.to::<u128>() as f64 / 1e18,
            Err(e) => {
                warn!(
                    token = %token.symbol,
                    error = %e,
                    "nad.fun Lens getAmountOut (sell) failed, using inverse"
                );
                1.0 / rate_mon_to_token
            }
        };

        let dex_name = if token.graduated {
            "nad.fun".to_string()
        } else {
            "nad.fun-bonding".to_string()
        };

        let pair = format!("MON/{}", token.symbol);

        info!(
            dex = %dex_name,
            token = %token.symbol,
            rate_mon_to_token = rate_mon_to_token,
            rate_token_to_mon = rate_token_to_mon,
            "nad.fun price quote"
        );

        // Liquidity is unknown without reserve data; set 0
        let liquidity_usd = 0.0;
        // nad.fun typical fee ~1%
        let fee_pct = 1.0;

        edges.push(PoolEdge {
            dex: dex_name.clone(),
            pair: pair.clone(),
            pool_address: lens_addr.to_string(),
            reserve0: U256::ZERO,
            reserve1: U256::ZERO,
            spot_price: rate_mon_to_token,
            liquidity_usd,
            fee_pct,
            token_in: "MON".to_string(),
            token_out: token.symbol.clone(),
            rate: rate_mon_to_token,
            source_chain: "mainnet".to_string(),
        });

        edges.push(PoolEdge {
            dex: dex_name,
            pair,
            pool_address: lens_addr.to_string(),
            reserve0: U256::ZERO,
            reserve1: U256::ZERO,
            spot_price: rate_mon_to_token,
            liquidity_usd,
            fee_pct,
            token_in: token.symbol.clone(),
            token_out: "MON".to_string(),
            rate: rate_token_to_mon,
            source_chain: "mainnet".to_string(),
        });
    }

    info!(count = edges.len(), "Fetched nad.fun pool edges");
    edges
}

/// Check if a nad.fun token has graduated to the DEX (convenience wrapper).
#[allow(dead_code)]
pub async fn is_graduated(
    provider: &MonadProvider,
    lens_addr: &str,
    token_addr: &str,
) -> Result<bool> {
    let lens = ILens::new(Address::from_str(lens_addr)?, provider.clone());
    let result = lens
        .isGraduated(Address::from_str(token_addr)?)
        .call()
        .await?;
    Ok(result._0)
}

// ──────────────────────────────────────────────────────────────────────────────
// Market intelligence structs for /market/nadfun
// ──────────────────────────────────────────────────────────────────────────────

/// Enriched token data for the market intelligence endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct NadFunMarketToken {
    pub symbol: String,
    pub address: String,
    pub graduated: bool,
    /// MON received per 1 token (sell quote from Lens)
    pub price_in_mon: f64,
    /// price_in_mon * mon_usdc_price
    pub price_in_usdc: f64,
    /// Tokens received per 1 MON (buy quote from Lens)
    pub tokens_per_mon: f64,
    /// "graduated" | "close_to_graduation" | "trending" | "active"
    pub signal: String,
    pub volume_24h: Option<f64>,
    pub market_cap: Option<f64>,
    /// Full API response object — AI can read any field it needs
    pub raw: Value,
}

/// Fetch enriched market token data for the /market/nadfun endpoint.
/// Returns as many tokens as the Lens answers successfully.
pub async fn fetch_nadfun_market_tokens(
    client: &reqwest::Client,
    provider: &MonadProvider,
    lens_addr: &str,
    api_url: &str,
    known_tokens: &[(String, String)],
    count: usize,
    mon_usdc_price: f64,
) -> Vec<NadFunMarketToken> {
    // 1. Fetch base token list from REST API
    let url = format!("{}?sort=volume&limit={}", api_url, count);
    info!(url = %url, "Fetching nad.fun tokens for market endpoint");

    let body: Value = match client.get(&url).send().await {
        Ok(r) => {
            let text = match r.text().await {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "nad.fun API read body failed");
                    return vec![];
                }
            };
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, preview = %&text[..text.len().min(200)], "nad.fun API JSON parse failed");
                    return vec![];
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "nad.fun API request failed");
            return vec![];
        }
    };

    let preview = {
        let raw_str = serde_json::to_string(&body).unwrap_or_default();
        raw_str[..raw_str.len().min(400)].to_string()
    };
    info!(preview = %preview, "nad.fun market API raw response (truncated)");

    // Extract token array
    let items: Vec<Value> = {
        if let Some(arr) = body.as_array() {
            arr.clone()
        } else {
            let mut found = vec![];
            for key in &["tokens", "data", "result", "items"] {
                if let Some(arr) = body.get(key).and_then(|v| v.as_array()) {
                    found = arr.clone();
                    break;
                }
            }
            found
        }
    };

    if items.is_empty() {
        if !known_tokens.is_empty() {
            warn!("nad.fun: API empty — querying {} known token(s) via Lens", known_tokens.len());
            return enrich_known_tokens(provider, lens_addr, known_tokens, count, mon_usdc_price).await;
        }
        warn!("nad.fun: no tokens from API and NADFUN_KNOWN_TOKENS not set");
        return vec![];
    }

    // Parse volume for signal ranking
    let mut volume_list: Vec<f64> = items
        .iter()
        .filter_map(|item| {
            item.get("volume24h")
                .or_else(|| item.get("volume_24h"))
                .or_else(|| item.get("volume"))
                .and_then(|v| v.as_f64())
        })
        .collect();
    volume_list.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let top3_volume_threshold = volume_list.get(2).copied().unwrap_or(0.0);

    // 2. Enrich each token via Lens
    let lens_address = match Address::from_str(lens_addr) {
        Ok(a) => a,
        Err(e) => {
            warn!(error = %e, "Invalid nad.fun Lens address for market endpoint");
            return vec![];
        }
    };
    let lens = ILens::new(lens_address, provider.clone());
    let one_mon = U256::from(1_000_000_000_000_000_000u128);
    let token_decimals = 18u32;
    let one_token = U256::from(10u128.pow(token_decimals));

    let mut results = Vec::new();

    for item in items.iter().take(count) {
        let address = item
            .get("address")
            .or_else(|| item.get("tokenAddress"))
            .or_else(|| item.get("contract"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if address.is_empty() {
            continue;
        }

        let symbol = item
            .get("symbol")
            .or_else(|| item.get("ticker"))
            .or_else(|| item.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN")
            .to_string();

        let graduated = item
            .get("graduated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let volume_24h = item
            .get("volume24h")
            .or_else(|| item.get("volume_24h"))
            .or_else(|| item.get("volume"))
            .and_then(|v| v.as_f64());

        let market_cap = item
            .get("marketCap")
            .or_else(|| item.get("market_cap"))
            .or_else(|| item.get("mcap"))
            .and_then(|v| v.as_f64());

        let bonding_curve_pct = item
            .get("bondingCurvePct")
            .or_else(|| item.get("bonding_curve_pct"))
            .or_else(|| item.get("progress"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let token_addr = match Address::from_str(&address) {
            Ok(a) => a,
            Err(_) => {
                warn!(token = %symbol, "Invalid token address, skipping market query");
                continue;
            }
        };

        // Buy quote: 1 MON → TOKEN
        let tokens_per_mon = match lens.getAmountOut(token_addr, one_mon, true).call().await {
            Ok(r) => r.amountOut.to::<u128>() as f64 / 10f64.powi(token_decimals as i32),
            Err(e) => {
                warn!(token = %symbol, error = %e, "Lens buy quote failed, skipping");
                continue;
            }
        };

        if tokens_per_mon <= 0.0 {
            warn!(token = %symbol, "Zero buy quote from Lens, skipping");
            continue;
        }

        // Sell quote: 1 TOKEN → MON
        let price_in_mon = match lens.getAmountOut(token_addr, one_token, false).call().await {
            Ok(r) => r.amountOut.to::<u128>() as f64 / 1e18,
            Err(_) => {
                // Fallback: inverse of buy rate
                1.0 / tokens_per_mon
            }
        };

        let price_in_usdc = price_in_mon * mon_usdc_price;

        // Signal assignment
        let signal = if graduated {
            "graduated".to_string()
        } else if bonding_curve_pct >= 80.0 {
            "close_to_graduation".to_string()
        } else if volume_24h.map(|v| v >= top3_volume_threshold && v > 0.0).unwrap_or(false) {
            "trending".to_string()
        } else {
            "active".to_string()
        };

        info!(
            token = %symbol,
            price_in_mon = price_in_mon,
            price_in_usdc = price_in_usdc,
            signal = %signal,
            "nad.fun market token enriched"
        );

        results.push(NadFunMarketToken {
            symbol,
            address,
            graduated,
            price_in_mon,
            price_in_usdc,
            tokens_per_mon,
            signal,
            volume_24h,
            market_cap,
            raw: item.clone(),
        });
    }

    info!(count = results.len(), "nad.fun market tokens fetched");
    results
}

/// Enrich a static list of known tokens via the Lens contract when the API returns no data.
/// Skips volume/mcap/bonding_curve data (not available without API).
async fn enrich_known_tokens(
    provider: &MonadProvider,
    lens_addr: &str,
    known_tokens: &[(String, String)],
    count: usize,
    mon_usdc_price: f64,
) -> Vec<NadFunMarketToken> {
    let lens_address = match Address::from_str(lens_addr) {
        Ok(a) => a,
        Err(e) => {
            warn!(error = %e, "Invalid nad.fun Lens address for known tokens");
            return vec![];
        }
    };
    let lens = ILens::new(lens_address, provider.clone());
    let one_mon = U256::from(1_000_000_000_000_000_000u128);
    let token_decimals = 18u32;
    let one_token = U256::from(10u128.pow(token_decimals));

    let mut results = Vec::new();

    for (symbol, address) in known_tokens.iter().take(count) {
        let token_addr = match Address::from_str(address) {
            Ok(a) => a,
            Err(_) => {
                warn!(token = %symbol, "Invalid known token address, skipping");
                continue;
            }
        };

        let graduated = lens
            .isGraduated(token_addr)
            .call()
            .await
            .map(|r| r._0)
            .unwrap_or(false);

        let tokens_per_mon = match lens.getAmountOut(token_addr, one_mon, true).call().await {
            Ok(r) => r.amountOut.to::<u128>() as f64 / 10f64.powi(token_decimals as i32),
            Err(e) => {
                warn!(token = %symbol, error = %e, "Lens buy quote failed for known token, skipping");
                continue;
            }
        };

        if tokens_per_mon <= 0.0 {
            warn!(token = %symbol, "Zero buy quote for known token, skipping");
            continue;
        }

        let price_in_mon = match lens.getAmountOut(token_addr, one_token, false).call().await {
            Ok(r) => r.amountOut.to::<u128>() as f64 / 1e18,
            Err(_) => 1.0 / tokens_per_mon,
        };

        let price_in_usdc = price_in_mon * mon_usdc_price;
        let signal = if graduated { "graduated".to_string() } else { "active".to_string() };

        info!(
            token = %symbol,
            price_in_mon = price_in_mon,
            signal = %signal,
            "nad.fun known token enriched via Lens"
        );

        results.push(NadFunMarketToken {
            symbol: symbol.clone(),
            address: address.clone(),
            graduated,
            price_in_mon,
            price_in_usdc,
            tokens_per_mon,
            signal,
            volume_24h: None,
            market_cap: None,
            raw: serde_json::json!({"symbol": symbol, "address": address, "source": "known_tokens"}),
        });
    }

    info!(count = results.len(), "nad.fun known tokens enriched via Lens");
    results
}
