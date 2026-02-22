#[allow(dead_code)]
/// Returns the pair set for a given wallet address (or default if not provided).
/// Uses the second byte of the address to deterministically assign a pair bucket.
pub fn get_pair_set(wallet: Option<&str>) -> Vec<(&'static str, &'static str)> {
    let wallet = match wallet {
        Some(w) if w.len() >= 4 => w,
        _ => return vec![("MON", "USDC")],
    };

    // Parse second byte: chars at index 2..4 (after "0x")
    let start = if wallet.starts_with("0x") || wallet.starts_with("0X") {
        2
    } else {
        0
    };

    let byte = u8::from_str_radix(&wallet[start..start + 2], 16).unwrap_or(0);

    match byte % 3 {
        0 => vec![("MON", "USDC"), ("WETH", "USDC")],
        1 => vec![("MON", "USDC"), ("WBTC", "USDC")],
        _ => vec![("MON", "USDC"), ("MON", "WETH")],
    }
}
