use std::fmt;

use crate::config::Config;
use crate::product::{parse_products, Product};

/// Any failure while reading from the WooCommerce Store API.
#[derive(Debug)]
pub struct UpstreamError(pub String);

impl fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UpstreamError {}

/// Fetch and normalize the upstream product list.
pub fn fetch_products(agent: &ureq::Agent, config: &Config) -> Result<Vec<Product>, UpstreamError> {
    let mut response = agent
        .get(config.upstream_url.as_str())
        .call()
        .map_err(|err| UpstreamError(format!("failed to reach upstream: {err}")))?;

    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|err| UpstreamError(format!("failed to read upstream body: {err}")))?;

    parse_products(&text)
        .map_err(|err| UpstreamError(format!("upstream returned invalid JSON: {err}")))
}
