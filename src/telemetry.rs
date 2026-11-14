use sentry::integrations::tracing::{BreadcrumbLayer, ErrorLayer, LogLayer, SpanLayer};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::config::Config;

/// Initialise Sentry and the tracing subscriber.
///
/// The returned guard must be kept alive for the lifetime of the process so
/// buffered Sentry events are flushed on shutdown.
pub fn init(config: &Config) -> sentry::ClientInitGuard {
    let traces_rate = config.sentry_traces_sample_rate as f32;

    let mut options = sentry::ClientOptions::new()
        .release(config.sentry_release.clone())
        .environment(config.sentry_environment.clone())
        .sample_rate(config.sentry_sample_rate as f32)
        // Skip health checks; sample everything else at the configured rate.
        .traces_sampler(move |ctx| {
            if ctx.name().contains("/health") {
                0.0
            } else {
                traces_rate
            }
        })
        .attach_stacktrace(true)
        .send_default_pii(false)
        .debug(config.sentry_debug);

    if let Some(raw) = config.sentry_dsn.as_deref() {
        match raw.parse() {
            Ok(dsn) => options.dsn = Some(dsn),
            Err(err) => eprintln!("invalid SENTRY_DSN, error reporting disabled: {err}"),
        }
    }

    let guard = sentry::init(options);

    // `RUST_LOG` controls console verbosity; default to `info`.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        // Spans become Sentry transactions/spans (performance tracing).
        .with(SpanLayer::new().with_filter(LevelFilter::INFO))
        // ERROR events become Sentry error events, with exception chains.
        .with(ErrorLayer::new().with_filter(LevelFilter::ERROR))
        // INFO+ events are also sent as Sentry structured logs.
        .with(LogLayer::new().with_filter(LevelFilter::INFO))
        // Lower-level events become breadcrumbs on later errors.
        .with(BreadcrumbLayer::new().with_filter(LevelFilter::INFO))
        .init();

    if config.sentry_dsn.is_none() {
        tracing::warn!("SENTRY_DSN is not set; Sentry is disabled");
    }

    guard
}
