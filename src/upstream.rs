use std::fmt;
use std::time::Instant;

use crate::config::Config;
use crate::product::{Product, parse_products};

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
#[tracing::instrument(
    skip(agent, config),
    name = "upstream.fetch",
    fields(upstream.url = %config.upstream_url)
)]
pub fn fetch_products(agent: &ureq::Agent, config: &Config) -> Result<Vec<Product>, UpstreamError> {
    let started = Instant::now();
    let result = fetch_products_inner(agent, config);
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

    sentry::metrics::distribution("upstream.fetch.duration", elapsed_ms)
        .unit(sentry::protocol::Unit::Millisecond)
        .capture();

    match &result {
        Ok(products) => {
            sentry::metrics::gauge("upstream.products", products.len() as f64).capture();
        }
        Err(_) => {
            sentry::metrics::counter("upstream.errors", 1.0).capture();
        }
    }

    result
}

fn fetch_products_inner(
    agent: &ureq::Agent,
    config: &Config,
) -> Result<Vec<Product>, UpstreamError> {
    // Continue the caller's trace on the upstream request, if there is one.
    let trace_headers: Vec<(&'static str, String)> = sentry::configure_scope(|scope| {
        scope
            .get_span()
            .map(|span| span.iter_headers().collect::<Vec<_>>())
            .unwrap_or_default()
    });

    let mut request = agent.get(config.upstream_url.as_str());
    for (name, value) in &trace_headers {
        request = request.header(*name, value.as_str());
    }

    let mut response = request
        .call()
        .map_err(|err| UpstreamError(format!("failed to reach upstream: {err}")))?;

    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|err| UpstreamError(format!("failed to read upstream body: {err}")))?;

    match parse_products(&text) {
        Ok(products) => Ok(products),
        Err(err) => {
            attach_upstream_body(&text);
            Err(UpstreamError(format!(
                "upstream returned invalid JSON: {err}"
            )))
        }
    }
}

/// Attach a truncated copy of the upstream body to the next error event so
/// malformed payloads can be inspected in Sentry.
fn attach_upstream_body(body: &str) {
    const MAX_ATTACHMENT_BYTES: usize = 64 * 1024;

    let truncated = if body.len() <= MAX_ATTACHMENT_BYTES {
        body
    } else {
        let mut end = MAX_ATTACHMENT_BYTES;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        &body[..end]
    };

    let attachment = sentry::protocol::Attachment {
        buffer: truncated.as_bytes().to_vec(),
        filename: "upstream-response.json".to_string(),
        content_type: Some("application/json".to_string()),
        ..Default::default()
    };

    sentry::configure_scope(|scope| scope.add_attachment(attachment));
}
