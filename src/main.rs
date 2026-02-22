mod api;
mod arbitrage;
mod config;
mod monad;
mod x402;

use std::sync::Arc;

use anyhow::Result;
use axum::{middleware, routing::get, Router};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;
use tracing_subscriber::EnvFilter;

use api::{
    dex_arb::{dex_arb_handler, AppState},
    health::health_handler,
    nadfun_market::nadfun_market_handler,
};
use config::Config;
use monad::rpc::create_provider;
use x402::x402_gate;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env()?;
    info!(
        rpc = %cfg.monad_rpc_url,
        nadfun_rpc = %cfg.nadfun_rpc_url,
        port = cfg.port,
        kuru_pool = %cfg.kuru_pool_mon_usdc,
        kuru_fee_pct = cfg.kuru_fee_pct,
        uv3_factory = %cfg.uniswap_v3_factory,
        uv3_fee_tier = cfg.uniswap_v3_fee_tier,
        nadfun_lens = %cfg.nadfun_lens,
        nadfun_top_tokens = cfg.nadfun_top_tokens_count,
        "xclaw-engine starting"
    );

    let provider = create_provider(&cfg.monad_rpc_url).await?;
    info!("Monad testnet RPC provider initialized");

    let nadfun_provider = create_provider(&cfg.nadfun_rpc_url).await?;
    info!("Monad mainnet (nad.fun) RPC provider initialized");

    let http_client = reqwest::Client::new();

    let state = Arc::new(AppState {
        provider,
        nadfun_provider,
        config: cfg.clone(),
        http_client,
    });

    let protected = Router::new()
        .route("/arbitrage/dex", get(dex_arb_handler))
        .route("/market/nadfun", get(nadfun_market_handler))
        .layer(middleware::from_fn_with_state(state.clone(), x402_gate));

    let app = Router::new()
        .route("/health", get(health_handler))
        .merge(protected)
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let addr = format!("0.0.0.0:{}", cfg.port);
    info!(addr = %addr, "Listening");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
