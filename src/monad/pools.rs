use alloy::primitives::{Address, Uint, U256};
use alloy::sol;
use anyhow::Result;
use std::str::FromStr;

use super::rpc::MonadProvider;

// Kuru MON/USDC pool — UniswapV2-style AMM vault interface
sol! {
    #[allow(missing_docs)]
    #[sol(rpc)]
    interface IUniswapV2Pair {
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast);
        function token0() external view returns (address);
        function token1() external view returns (address);
    }
}

// Kuru OrderBook — CLOB, price via bestBidAsk() + getMarketParams()
sol! {
    #[allow(missing_docs)]
    #[sol(rpc)]
    interface IKuruOrderBook {
        function bestBidAsk() external view returns (uint256 bid, uint256 ask);
        function getMarketParams() external view returns (
            uint32 pricePrecision,
            uint96 sizePrecision,
            address baseAsset,
            uint256 baseDecimals,
            address quoteAsset,
            uint256 quoteDecimals,
            uint32 tickSize,
            uint96 minSize,
            uint96 maxSize,
            uint256 takerFeeBps,
            uint256 makerFeeBps
        );
    }
}

// Uniswap V3 interfaces
sol! {
    #[allow(missing_docs)]
    #[sol(rpc)]
    interface IUniswapV3Factory {
        function getPool(address tokenA, address tokenB, uint24 fee)
            external view returns (address pool);
    }

    #[allow(missing_docs)]
    #[sol(rpc)]
    interface IUniswapV3Pool {
        function slot0() external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint16 observationIndex,
            uint16 observationCardinality,
            uint16 observationCardinalityNext,
            uint8 feeProtocol,
            bool unlocked
        );
        function liquidity() external view returns (uint128);
    }
}

#[derive(Debug, Clone)]
pub struct PoolSnapshot {
    pub dex: String,
    pub pair: String,
    #[allow(dead_code)]
    pub pool_address: String,
    #[allow(dead_code)]
    pub reserve0: U256,
    #[allow(dead_code)]
    pub reserve1: U256,
    /// price of token0 in terms of token1 (MON price in USDC) — kept for backward compat
    pub spot_price: f64,
    /// liquidity depth in USD (approximated)
    pub liquidity_usd: f64,
    /// trading fee in percent (e.g. 0.1 for 0.1%, 0.3 for 0.3%)
    pub fee_pct: f64,
    /// symbol of the input token for this directed edge (e.g. "MON")
    pub token_in: String,
    /// symbol of the output token for this directed edge (e.g. "USDC")
    pub token_out: String,
    /// how much token_out per 1.0 token_in at spot price (pre-fee)
    pub rate: f64,
    /// chain this pool was fetched from: "testnet" or "mainnet"
    pub source_chain: String,
}

/// Directed graph edge alias for clarity in graph code.
pub type PoolEdge = PoolSnapshot;

#[allow(dead_code)]
pub async fn get_reserves(
    provider: &MonadProvider,
    pool_addr: Address,
) -> Result<(U256, U256)> {
    let contract = IUniswapV2Pair::new(pool_addr, provider.clone());
    let result = contract.getReserves().call().await?;
    Ok((U256::from(result.reserve0), U256::from(result.reserve1)))
}

#[allow(dead_code)]
pub fn spot_price(reserve0: U256, reserve1: U256) -> f64 {
    // Returns price of token0 in terms of token1
    // reserve0 = MON (18 decimals), reserve1 = USDC (6 decimals)
    // price = (reserve1 / 1e6) / (reserve0 / 1e18) = USDC per MON
    let r0 = reserve0.to::<u128>() as f64 / 1e18;
    let r1 = reserve1.to::<u128>() as f64 / 1e6;
    if r0 == 0.0 {
        return 0.0;
    }
    r1 / r0
}

/// Convert Uniswap V3 sqrtPriceX96 limbs to spot price (USDC per WMON).
///
/// sqrtPriceX96 = sqrt(token1_raw / token0_raw) * 2^96
/// token0 = WMON (18 dec), token1 = USDC (6 dec)
pub fn v3_spot_price(sqrt_limbs: [u64; 3], token0_is_wmon: bool) -> f64 {
    // Reconstruct value from lower 128 bits (sufficient for all realistic prices)
    let lower_u128: u128 =
        (sqrt_limbs[0] as u128) | ((sqrt_limbs[1] as u128) << 64);
    let sqrt_ratio = lower_u128 as f64 / 2f64.powi(96);
    let price_raw = sqrt_ratio * sqrt_ratio; // token1_raw / token0_raw

    if token0_is_wmon {
        // price_human = price_raw * 1e18 / 1e6 = price_raw * 1e12 → USDC per WMON
        price_raw * 1e12
    } else {
        // token0=USDC, token1=WMON → invert and adjust decimals
        if price_raw == 0.0 {
            0.0
        } else {
            (1.0 / price_raw) / 1e12
        }
    }
}

/// Fetch a V2 AMM pool snapshot using getReserves(). Kept for future non-Kuru AMM pools.
#[allow(dead_code)]
pub async fn fetch_pool_snapshot(
    provider: &MonadProvider,
    dex: &str,
    pair: &str,
    pool_address: &str,
    fee_pct: f64,
) -> Result<Vec<PoolSnapshot>> {
    let addr = Address::from_str(pool_address)?;
    let (reserve0, reserve1) = get_reserves(provider, addr).await?;
    let price = spot_price(reserve0, reserve1); // USDC per MON

    // Liquidity approximated as 2x the USDC side (reserve1 / 1e6)
    let usdc_side = reserve1.to::<u128>() as f64 / 1e6;
    let liquidity_usd = 2.0 * usdc_side;

    // Parse token symbols from pair string (e.g. "MON/USDC")
    let parts: Vec<&str> = pair.split('/').collect();
    let (sym0, sym1) = if parts.len() == 2 {
        (parts[0], parts[1])
    } else {
        ("MON", "USDC")
    };

    let forward = PoolSnapshot {
        dex: dex.to_string(),
        pair: pair.to_string(),
        pool_address: pool_address.to_string(),
        reserve0,
        reserve1,
        spot_price: price,
        liquidity_usd,
        fee_pct,
        token_in: sym0.to_string(),
        token_out: sym1.to_string(),
        rate: price,
        source_chain: "testnet".to_string(),
    };

    let reverse = PoolSnapshot {
        dex: dex.to_string(),
        pair: pair.to_string(),
        pool_address: pool_address.to_string(),
        reserve0,
        reserve1,
        spot_price: price,
        liquidity_usd,
        fee_pct,
        token_in: sym1.to_string(),
        token_out: sym0.to_string(),
        rate: if price > 0.0 { 1.0 / price } else { 0.0 },
        source_chain: "testnet".to_string(),
    };

    Ok(vec![forward, reverse])
}

/// Fetch a Uniswap V3 pool snapshot using slot0() + liquidity().
/// Returns two directed edges: forward and reverse.
///
/// `token0` must be the lexicographically smaller address (WMON),
/// `token1` the larger (USDC) — matching how V3 factory orders tokens.
/// If `pool_override` is provided, the factory lookup is skipped.
pub async fn fetch_v3_pool_snapshot(
    provider: &MonadProvider,
    dex: &str,
    pair: &str,
    factory_addr: &str,
    token0: &str,
    token1: &str,
    fee: u32,
    pool_override: Option<&str>,
    fee_pct: f64,
) -> Result<Vec<PoolSnapshot>> {
    // Resolve pool address
    let pool_address = if let Some(addr) = pool_override {
        addr.to_string()
    } else {
        let factory = IUniswapV3Factory::new(
            Address::from_str(factory_addr)?,
            provider.clone(),
        );
        let fee_u24 = Uint::<24, 1>::from(fee);
        let t0 = Address::from_str(token0)?;
        let t1 = Address::from_str(token1)?;
        let result = factory.getPool(t0, t1, fee_u24).call().await?;
        let addr = result.pool;
        if addr == Address::ZERO {
            anyhow::bail!(
                "Uniswap V3 pool not found via factory for fee tier {}",
                fee
            );
        }
        format!("{:#x}", addr)
    };

    let pool_addr = Address::from_str(&pool_address)?;
    let pool = IUniswapV3Pool::new(pool_addr, provider.clone());

    let s0 = pool.slot0().call().await?;
    let liq = pool.liquidity().call().await?;

    // Extract sqrtPriceX96 limbs (little-endian: limbs[0] = least significant)
    let sl = s0.sqrtPriceX96.as_limbs();
    let sqrt_limbs = [sl[0], sl[1], sl[2]];

    // WMON (0x3bd...) < USDC (0x534...) by address → token0 = WMON
    let price = v3_spot_price(sqrt_limbs, true);

    // Rough USD liquidity: 2 * L * sqrtRatio / 2^96 / 1e6
    let sqrt_ratio =
        ((sqrt_limbs[0] as u128) | ((sqrt_limbs[1] as u128) << 64)) as f64
            / 2f64.powi(96);
    let l = liq._0 as f64;
    let usdc_side = l * sqrt_ratio / 1e6;
    let liquidity_usd = 2.0 * usdc_side;

    // Parse token symbols from pair string
    let parts: Vec<&str> = pair.split('/').collect();
    let (sym0, sym1) = if parts.len() == 2 {
        (parts[0], parts[1])
    } else {
        ("MON", "USDC")
    };

    let forward = PoolSnapshot {
        dex: dex.to_string(),
        pair: pair.to_string(),
        pool_address: pool_address.clone(),
        reserve0: U256::ZERO,
        reserve1: U256::ZERO,
        spot_price: price,
        liquidity_usd,
        fee_pct,
        token_in: sym0.to_string(),
        token_out: sym1.to_string(),
        rate: price,
        source_chain: "testnet".to_string(),
    };

    let reverse = PoolSnapshot {
        dex: dex.to_string(),
        pair: pair.to_string(),
        pool_address,
        reserve0: U256::ZERO,
        reserve1: U256::ZERO,
        spot_price: price,
        liquidity_usd,
        fee_pct,
        token_in: sym1.to_string(),
        token_out: sym0.to_string(),
        rate: if price > 0.0 { 1.0 / price } else { 0.0 },
        source_chain: "testnet".to_string(),
    };

    Ok(vec![forward, reverse])
}

/// Fetch a Kuru OrderBook (CLOB) pool snapshot.
/// Price = midpoint of (bid + ask) / pricePrecision.
/// Fee = takerFeeBps / 10000 * 100 (bps → percent).
/// Returns two directed edges: forward (token0→token1) and reverse.
pub async fn fetch_kuru_pool_snapshot(
    provider: &MonadProvider,
    dex: &str,
    pair: &str,
    pool_address: &str,
) -> Result<Vec<PoolSnapshot>> {
    let addr = Address::from_str(pool_address)?;
    let book = IKuruOrderBook::new(addr, provider.clone());

    // getMarketParams() may not exist on older testnet deployments — fall back to safe defaults.
    // MON/USDC price precision = 1_000_000 (USDC has 6 decimals), taker fee = 0.1%.
    let (price_precision, fee_pct) = match book.getMarketParams().call().await {
        Ok(params) => {
            let pp = params.pricePrecision as f64;
            let fee = params.takerFeeBps.to::<u128>() as f64 / 10000.0 * 100.0;
            (pp, fee)
        }
        Err(e) => {
            tracing::warn!(
                pool = pool_address,
                error = %e,
                "Kuru getMarketParams failed — using defaults (pricePrecision=1e6, fee=0.1%)"
            );
            (1_000_000.0, 0.1)
        }
    };
    if price_precision == 0.0 {
        anyhow::bail!("Kuru pricePrecision is zero");
    }

    let ba = book.bestBidAsk().call().await?;
    let bid_raw = ba.bid.to::<u128>() as f64;
    let ask_raw = ba.ask.to::<u128>() as f64;

    let price = if bid_raw > 0.0 && ask_raw > 0.0 {
        ((bid_raw + ask_raw) / 2.0) / price_precision
    } else if ask_raw > 0.0 {
        ask_raw / price_precision
    } else if bid_raw > 0.0 {
        bid_raw / price_precision
    } else {
        anyhow::bail!("Kuru pool has no orders (bid=0, ask=0)");
    };

    // Sanity check: MON is a sub-$1 asset on Monad right now.
    // Reject clearly stale/misconfigured Kuru prices so they don't pollute the arb detector.
    if price > 500.0 || price < 0.000_001 {
        anyhow::bail!(
            "Kuru price {price:.6} USDC/MON out of sane range (bid={bid_raw}, ask={ask_raw}, pp={price_precision})"
        );
    }

    let parts: Vec<&str> = pair.split('/').collect();
    let (sym0, sym1) = if parts.len() == 2 { (parts[0], parts[1]) } else { ("MON", "USDC") };

    let forward = PoolSnapshot {
        dex: dex.to_string(),
        pair: pair.to_string(),
        pool_address: pool_address.to_string(),
        reserve0: U256::ZERO,
        reserve1: U256::ZERO,
        spot_price: price,
        liquidity_usd: 0.0,
        fee_pct,
        token_in: sym0.to_string(),
        token_out: sym1.to_string(),
        rate: price,
        source_chain: "testnet".to_string(),
    };
    let reverse = PoolSnapshot {
        dex: dex.to_string(),
        pair: pair.to_string(),
        pool_address: pool_address.to_string(),
        reserve0: U256::ZERO,
        reserve1: U256::ZERO,
        spot_price: price,
        liquidity_usd: 0.0,
        fee_pct,
        token_in: sym1.to_string(),
        token_out: sym0.to_string(),
        rate: if price > 0.0 { 1.0 / price } else { 0.0 },
        source_chain: "testnet".to_string(),
    };
    Ok(vec![forward, reverse])
}
