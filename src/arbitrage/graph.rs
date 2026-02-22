#![allow(dead_code)]

use std::collections::HashMap;

use serde::Serialize;

use crate::monad::pools::PoolEdge;

#[derive(Debug, Clone, Serialize)]
pub struct ArbPath {
    pub tokens: Vec<String>,      // e.g. ["USDC", "MON", "NADTOKEN", "USDC"]
    pub dexes: Vec<String>,       // e.g. ["Uniswap V3", "nad.fun", "Kuru"]
    pub hops: usize,
    pub gross_profit_pct: f64,
    pub net_profit_pct: f64,
    pub liquidity_usd: f64,
    pub confidence: f64,
    pub action: String,
}

/// Build a directed adjacency map: token_in symbol → outgoing edges.
pub fn build_graph<'a>(edges: &'a [PoolEdge]) -> HashMap<String, Vec<&'a PoolEdge>> {
    let mut graph: HashMap<String, Vec<&'a PoolEdge>> = HashMap::new();
    for edge in edges {
        if edge.rate > 0.0 {
            graph.entry(edge.token_in.clone()).or_default().push(edge);
        }
    }
    graph
}

/// Find circular arbitrage paths starting and ending at `start`.
/// DFS up to `max_hops` depth. Returns only net-profitable paths, sorted desc.
pub fn find_arb_paths(
    graph: &HashMap<String, Vec<&PoolEdge>>,
    start: &str,
    max_hops: usize,
) -> Vec<ArbPath> {
    let mut results = Vec::new();
    let mut path_tokens = vec![start.to_string()];
    let mut path_edges: Vec<&PoolEdge> = Vec::new();

    dfs(
        graph,
        start,
        start,
        max_hops,
        &mut path_tokens,
        &mut path_edges,
        &mut results,
    );

    results.sort_by(|a, b| {
        b.net_profit_pct
            .partial_cmp(&a.net_profit_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

fn dfs<'a>(
    graph: &'a HashMap<String, Vec<&'a PoolEdge>>,
    current: &str,
    start: &str,
    remaining_hops: usize,
    path_tokens: &mut Vec<String>,
    path_edges: &mut Vec<&'a PoolEdge>,
    results: &mut Vec<ArbPath>,
) {
    // We've made at least one hop and returned to start — evaluate the cycle
    if !path_edges.is_empty() && current == start {
        let gross_rate: f64 = path_edges.iter().map(|e| e.rate).product();
        let gross_profit_pct = (gross_rate - 1.0) * 100.0;
        let fee_total_pct: f64 = path_edges.iter().map(|e| e.fee_pct).sum();
        let net_profit_pct = gross_profit_pct - fee_total_pct;

        if net_profit_pct > 0.0 {
            let hops = path_edges.len();

            // Min liquidity along path (ignore zeros — unknown means unconstrained)
            let known_liquidity: Vec<f64> = path_edges
                .iter()
                .map(|e| e.liquidity_usd)
                .filter(|&l| l > 0.0)
                .collect();
            let liquidity_usd = if known_liquidity.is_empty() {
                0.0
            } else {
                known_liquidity.iter().cloned().fold(f64::INFINITY, f64::min)
            };

            let confidence = confidence_from_liquidity(liquidity_usd);
            let dexes: Vec<String> = path_edges.iter().map(|e| e.dex.clone()).collect();
            let action = build_action_string(path_tokens, &dexes);

            results.push(ArbPath {
                tokens: path_tokens.clone(),
                dexes,
                hops,
                gross_profit_pct,
                net_profit_pct,
                liquidity_usd,
                confidence,
                action,
            });
        }
        return;
    }

    if remaining_hops == 0 {
        return;
    }

    // Early prune: if partial rate is already extremely low, abandon branch
    if !path_edges.is_empty() {
        let partial_rate: f64 = path_edges.iter().map(|e| e.rate).product();
        if partial_rate < 0.001 {
            return;
        }
    }

    let neighbors = match graph.get(current) {
        Some(n) => n,
        None => return,
    };

    for edge in neighbors {
        let next = &edge.token_out;

        // Don't revisit intermediate tokens (except returning to start)
        if next != start && path_tokens.contains(next) {
            continue;
        }

        // Don't use the same directed edge twice
        if path_edges
            .iter()
            .any(|e| e.pool_address == edge.pool_address && e.token_in == edge.token_in)
        {
            continue;
        }

        path_tokens.push(next.clone());
        path_edges.push(edge);

        dfs(
            graph,
            next,
            start,
            remaining_hops - 1,
            path_tokens,
            path_edges,
            results,
        );

        path_tokens.pop();
        path_edges.pop();
    }
}

fn build_action_string(tokens: &[String], dexes: &[String]) -> String {
    let mut parts = Vec::new();
    for (i, dex) in dexes.iter().enumerate() {
        if i + 1 < tokens.len() {
            parts.push(format!("{} → {} on {}", tokens[i], tokens[i + 1], dex));
        }
    }
    parts.join(", then ")
}

fn confidence_from_liquidity(liquidity_usd: f64) -> f64 {
    if liquidity_usd == 0.0 {
        // Unknown liquidity (e.g. nad.fun tokens) — assign low-mid confidence
        return 0.3;
    }
    let x = liquidity_usd / 10_000.0;
    let c = 0.95 * (1.0 - (-x).exp());
    c.clamp(0.1, 0.95)
}
