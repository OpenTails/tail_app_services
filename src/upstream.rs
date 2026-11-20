use std::collections::HashSet;
use std::fmt;
use std::time::Instant;

use crate::config::Config;
use crate::post::{Post, parse_posts};
use crate::product::{Product, apply_sale_items, parse_products};
use crate::sale::parse_sale_items;
use crate::thumb::ThumbhashCache;

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
    skip(agent, config, thumbhashes),
    name = "upstream.fetch",
    fields(upstream.url = %config.upstream_url)
)]
pub fn fetch_products(
    agent: &ureq::Agent,
    config: &Config,
    thumbhashes: &ThumbhashCache,
) -> Result<Vec<Product>, UpstreamError> {
    let started = Instant::now();
    let result = fetch_products_inner(agent, config, thumbhashes);
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
    thumbhashes: &ThumbhashCache,
) -> Result<Vec<Product>, UpstreamError> {
    let text = fetch_text(agent, &config.upstream_url)?;

    match parse_products(&text) {
        Ok(mut products) => {
            apply_sale_items(&mut products, &fetch_sale_ids(agent, config));
            if config.thumbhash_enabled {
                let urls: Vec<Option<String>> =
                    products.iter().map(|p| p.thumbnail.clone()).collect();
                for (product, hash) in products.iter_mut().zip(thumbhashes.resolve(agent, &urls)) {
                    product.thumbhash = hash;
                }
            }
            Ok(products)
        }
        Err(err) => {
            attach_upstream_body(&text);
            Err(UpstreamError(format!(
                "upstream returned invalid JSON: {err}"
            )))
        }
    }
}

/// Fetch the ids of the products that are on sale.
///
/// A failure here degrades to an empty set rather than failing the product
/// list: showing products without a sale badge is better than showing none, and
/// the error is logged and reported to Sentry.
fn fetch_sale_ids(agent: &ureq::Agent, config: &Config) -> HashSet<u64> {
    let text = match fetch_text(agent, &config.sale_items_url) {
        Ok(text) => text,
        Err(err) => {
            tracing::error!(
                error = %err,
                url = %config.sale_items_url,
                "sale-items fetch failed; treating no products as on sale"
            );
            return HashSet::new();
        }
    };

    match parse_sale_items(&text) {
        Ok(ids) => ids,
        Err(err) => {
            attach_upstream_body(&text);
            tracing::error!(
                error = %err,
                url = %config.sale_items_url,
                "sale-items returned invalid JSON; treating no products as on sale"
            );
            HashSet::new()
        }
    }
}

/// Fetch and normalize the upstream blog post list.
#[tracing::instrument(
    skip(agent, config, thumbhashes),
    name = "upstream.fetch",
    fields(upstream.url = %config.blog_upstream_url)
)]
pub fn fetch_posts(
    agent: &ureq::Agent,
    config: &Config,
    thumbhashes: &ThumbhashCache,
) -> Result<Vec<Post>, UpstreamError> {
    let started = Instant::now();
    let result = fetch_posts_inner(agent, config, thumbhashes);
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

    sentry::metrics::distribution("upstream.fetch.duration", elapsed_ms)
        .unit(sentry::protocol::Unit::Millisecond)
        .capture();

    match &result {
        Ok(posts) => {
            sentry::metrics::gauge("upstream.posts", posts.len() as f64).capture();
        }
        Err(_) => {
            sentry::metrics::counter("upstream.errors", 1.0).capture();
        }
    }

    result
}

fn fetch_posts_inner(
    agent: &ureq::Agent,
    config: &Config,
    thumbhashes: &ThumbhashCache,
) -> Result<Vec<Post>, UpstreamError> {
    let text = fetch_text(agent, &config.blog_upstream_url)?;

    match parse_posts(&text) {
        Ok(mut posts) => {
            if config.thumbhash_enabled {
                let urls: Vec<Option<String>> = posts.iter().map(|p| p.thumbnail.clone()).collect();
                for (post, hash) in posts.iter_mut().zip(thumbhashes.resolve(agent, &urls)) {
                    post.thumbhash = hash;
                }
            }
            Ok(posts)
        }
        Err(err) => {
            attach_upstream_body(&text);
            Err(UpstreamError(format!(
                "upstream returned invalid JSON: {err}"
            )))
        }
    }
}

/// GET a URL, continuing the caller's trace, and return the body as text.
fn fetch_text(agent: &ureq::Agent, url: &str) -> Result<String, UpstreamError> {
    // Continue the caller's trace on the upstream request, if there is one.
    let trace_headers: Vec<(&'static str, String)> = sentry::configure_scope(|scope| {
        scope
            .get_span()
            .map(|span| span.iter_headers().collect::<Vec<_>>())
            .unwrap_or_default()
    });

    let mut request = agent.get(url);
    for (name, value) in &trace_headers {
        request = request.header(*name, value.as_str());
    }

    let mut response = request
        .call()
        .map_err(|err| UpstreamError(format!("failed to reach upstream: {err}")))?;

    response
        .body_mut()
        .read_to_string()
        .map_err(|err| UpstreamError(format!("failed to read upstream body: {err}")))
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
