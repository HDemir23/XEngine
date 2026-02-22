use alloy::providers::{Provider, ProviderBuilder};
use alloy::transports::http::Http;
use anyhow::Result;
use reqwest::Client;
use std::sync::Arc;

pub type MonadProvider = Arc<
    alloy::providers::RootProvider<Http<Client>>,
>;

pub async fn create_provider(rpc_url: &str) -> Result<MonadProvider> {
    let provider = ProviderBuilder::new()
        .on_http(rpc_url.parse()?);
    Ok(Arc::new(provider))
}

pub async fn get_block_number(provider: &MonadProvider) -> Result<u64> {
    let block_num = provider.get_block_number().await?;
    Ok(block_num)
}
