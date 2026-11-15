mod cache;
mod config;
mod post;
mod product;
mod rng;
mod select;
mod server;
mod telemetry;
mod upstream;

use std::net::TcpListener;
use std::sync::Arc;

use config::Config;

fn main() {
    // Load `.env` if present. Real environment variables take precedence.
    let _ = dotenvy::dotenv();

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("configuration error: {message}");
            std::process::exit(1);
        }
    };

    // Keep the guard alive for the whole process so buffered events flush on exit.
    let _sentry_guard = telemetry::init(&config);

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(config.upstream_timeout))
        .build()
        .into();

    let app = Arc::new(server::App {
        cache: cache::ProductCache::new(config.cache_ttl, config.cache_max_stale),
        blog_cache: cache::PostCache::new(config.cache_ttl, config.cache_max_stale),
        agent,
        config: config.clone(),
    });

    let address = format!("0.0.0.0:{}", config.port);
    let listener = match TcpListener::bind(&address) {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!(error = %err, "failed to bind {}", address);
            std::process::exit(1);
        }
    };

    let local = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or(address);

    tracing::info!(
        address = %local,
        upstream = %config.upstream_url,
        cache_ttl_ms = config.cache_ttl.as_millis() as u64,
        cache_max_stale_ms = config.cache_max_stale.as_millis() as u64,
        default_count = config.default_count,
        "tailcompany-wp-proxy listening"
    );

    if let Err(err) = server::run(listener, app) {
        tracing::error!(error = %err, "server error");
        std::process::exit(1);
    }
}
