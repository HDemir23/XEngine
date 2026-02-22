use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
    middleware::Next,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

use crate::{api::dex_arb::AppState, config::Config};

const FACILITATOR_URL: &str = "https://x402-facilitator.molandak.org";
const USDC_ADDRESS: &str = "0x534b2f3A21130d7a60830c2Df862319e593943A3";
const NETWORK: &str = "eip155:10143";

// ── Serializable types ────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[allow(non_snake_case)]
pub(crate) struct PaymentOption {
    scheme: String,
    network: String,
    maxAmountRequired: String,
    resource: String,
    description: String,
    mimeType: String,
    payTo: String,
    maxTimeoutSeconds: u32,
    asset: String,
    extra: UsdcExtra,
}

#[derive(Serialize, Deserialize, Clone)]
struct UsdcExtra {
    name: String,
    version: String,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
pub(crate) struct PaymentRequirements {
    x402Version: u8,
    accepts: Vec<PaymentOption>,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct SettleRequest {
    x402Version: u8,
    payload: Value,
    paymentRequirements: PaymentOption,
}

#[derive(Deserialize)]
#[allow(non_snake_case, dead_code)]
struct SettleResponse {
    success: Option<bool>,
    isValid: Option<bool>,
    payer: Option<String>,
    error: Option<String>,
}

// ── Core helpers ──────────────────────────────────────────────────────────────

/// Return payment requirements for `path`, or None if the path is free.
pub(crate) fn requirements_for(path: &str, host: &str, cfg: &Config) -> Option<PaymentRequirements> {
    let (amount, description) = match path {
        "/arbitrage/dex" => ("100000", "DEX arbitrage market intelligence"),
        "/market/nadfun" => ("100000", "nad.fun market intelligence"),
        _ => return None,
    };

    let resource = format!("http://{}{}", host, path);

    let option = PaymentOption {
        scheme: "exact".to_string(),
        network: NETWORK.to_string(),
        maxAmountRequired: amount.to_string(),
        resource,
        description: description.to_string(),
        mimeType: "application/json".to_string(),
        payTo: cfg.pay_to_address.clone(),
        maxTimeoutSeconds: 300,
        asset: USDC_ADDRESS.to_string(),
        extra: UsdcExtra {
            name: "USDC".to_string(),
            version: "2".to_string(),
        },
    };

    Some(PaymentRequirements {
        x402Version: 2,
        accepts: vec![option],
    })
}

/// Decode a Base64-encoded payment-signature header value into JSON.
fn decode_sig(header: &str) -> Option<Value> {
    let bytes = B64.decode(header.trim()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// POST /settle to the facilitator.
/// Returns Ok(payer_address) on success or Err(reason) on failure.
pub(crate) async fn settle(
    client: &reqwest::Client,
    sig_header: &str,
    requirements: &PaymentOption,
) -> Result<String, String> {
    let payload = decode_sig(sig_header).ok_or_else(|| "invalid base64/JSON in payment-signature".to_string())?;

    let body = SettleRequest {
        x402Version: 2,
        payload,
        paymentRequirements: requirements.clone(),
    };

    let resp = client
        .post(format!("{}/settle", FACILITATOR_URL))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("facilitator unreachable: {e}"))?;

    let status = resp.status();
    let json: SettleResponse = resp
        .json()
        .await
        .map_err(|e| format!("invalid facilitator response: {e}"))?;

    if !status.is_success() {
        return Err(json.error.unwrap_or_else(|| format!("facilitator returned {status}")));
    }

    let ok = json.success.unwrap_or(false) || json.isValid.unwrap_or(false);
    if !ok {
        return Err(json.error.unwrap_or_else(|| "payment not valid".to_string()));
    }

    Ok(json.payer.unwrap_or_default())
}

// ── Axum middleware ───────────────────────────────────────────────────────────

pub async fn x402_gate(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    if !state.config.x402_enabled {
        return next.run(req).await;
    }

    let path = req.uri().path().to_string();
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost:8080")
        .to_string();

    let Some(reqs) = requirements_for(&path, &host, &state.config) else {
        return next.run(req).await;
    };

    let sig = req
        .headers()
        .get("payment-signature")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());

    let Some(sig) = sig else {
        return make_402(&reqs);
    };

    match settle(&state.http_client, &sig, &reqs.accepts[0]).await {
        Ok(payer) => {
            info!(payer = %payer, path = %path, "x402 payment settled");
            next.run(req).await
        }
        Err(reason) => {
            warn!(reason = %reason, path = %path, "x402 settle failed");
            make_402(&reqs)
        }
    }
}

fn make_402(reqs: &PaymentRequirements) -> Response<Body> {
    let json = serde_json::to_string(reqs).unwrap_or_default();
    let encoded = B64.encode(&json);

    Response::builder()
        .status(StatusCode::PAYMENT_REQUIRED)
        .header("payment-required", encoded)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"error":"Payment required","x402Version":2}"#))
        .unwrap()
}
