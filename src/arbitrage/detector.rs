use crate::monad::pools::PoolSnapshot;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ArbOpportunity {
    pub pair: String,
    pub dex_buy: String,
    pub dex_sell: String,
    pub price_buy: f64,
    pub price_sell: f64,
    pub spread_pct: f64,
    pub net_profit_pct: f64,
    pub liquidity_usd: f64,
    pub action: String,
    pub confidence: f64,
}

/// Find arbitrage opportunities across a set of pool snapshots for the same pair.
/// Compares every (buy, sell) pair combination; each pool carries its own fee_pct.
pub fn detect_opportunities(pools: &[PoolSnapshot]) -> Vec<ArbOpportunity> {
    let mut opportunities = Vec::new();

    for i in 0..pools.len() {
        for j in 0..pools.len() {
            if i == j {
                continue;
            }
            let buy_pool = &pools[i];
            let sell_pool = &pools[j];

            // Only compare same pair across different DEXes
            if buy_pool.pair != sell_pool.pair {
                continue;
            }
            if buy_pool.dex == sell_pool.dex {
                continue;
            }

            let price_buy = buy_pool.spot_price;
            let price_sell = sell_pool.spot_price;

            if price_buy <= 0.0 || price_sell <= 0.0 {
                continue;
            }

            let spread_pct = (price_sell - price_buy) / price_buy * 100.0;

            // Subtract each pool's trading fee (one swap on each side)
            let net_profit_pct = spread_pct - (buy_pool.fee_pct + sell_pool.fee_pct);

            if spread_pct <= 0.0 {
                continue;
            }

            let liquidity_usd = buy_pool.liquidity_usd.min(sell_pool.liquidity_usd);

            // Confidence: function of liquidity depth (sigmoid-ish, capped at 0.95)
            let confidence = confidence_from_liquidity(liquidity_usd);

            let action = format!(
                "BUY {} on {}, SELL on {}",
                buy_pool.pair, buy_pool.dex, sell_pool.dex
            );

            opportunities.push(ArbOpportunity {
                pair: buy_pool.pair.clone(),
                dex_buy: buy_pool.dex.clone(),
                dex_sell: sell_pool.dex.clone(),
                price_buy,
                price_sell,
                spread_pct,
                net_profit_pct,
                liquidity_usd,
                action,
                confidence,
            });
        }
    }

    // Sort by net_profit_pct descending
    opportunities.sort_by(|a, b| b.net_profit_pct.partial_cmp(&a.net_profit_pct).unwrap());
    opportunities
}

fn confidence_from_liquidity(liquidity_usd: f64) -> f64 {
    // Scale: $1k → ~0.5, $10k → ~0.75, $100k → ~0.92, $1M → ~0.95
    let x = liquidity_usd / 10_000.0;
    let c = 0.95 * (1.0 - (-x).exp());
    c.clamp(0.1, 0.95)
}
