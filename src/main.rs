mod cache;
mod config;
mod product;
mod rng;
mod select;
mod server;
mod upstream;

use std::net::TcpListener;
use std::sync::Arc;

use config::Config;

fn main() {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("configuration error: {message}");
            std::process::exit(1);
        }
    };

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(config.upstream_timeout))
        .build()
        .into();

    let app = Arc::new(server::App {
        cache: cache::ProductCache::new(config.cache_ttl, config.cache_max_stale),
        agent,
        config: config.clone(),
    });

    let address = format!("0.0.0.0:{}", config.port);
    let listener = match TcpListener::bind(&address) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("failed to bind {address}: {err}");
            std::process::exit(1);
        }
    };

    let local = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or(address);

    println!("tailcompany-wp-proxy listening on http://{local}");
    println!("  upstream:   {}", config.upstream_url);
    println!(
        "  cache:      TTL {}ms, stale-if-error {}ms",
        config.cache_ttl.as_millis(),
        config.cache_max_stale.as_millis()
    );
    println!(
        "  selection:  {} products, on-sale items first",
        config.default_count
    );

    if let Err(err) = server::run(listener, app) {
        eprintln!("server error: {err}");
        std::process::exit(1);
    }
}
