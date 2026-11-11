use std::env;
use std::time::Duration;

pub const DEFAULT_UPSTREAM_URL: &str =
    "https://thetailcompany.com/wp-json/wc/store/v1/products?tag=select&per_page=100";

/// Runtime configuration, read from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub upstream_url: String,
    pub upstream_timeout: Duration,
    pub cache_ttl: Duration,
    pub cache_max_stale: Duration,
    pub default_count: usize,
    pub max_count: usize,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Ok(Config {
            port: parse_u64("PORT", 3000, 0, u16::MAX as u64)? as u16,
            upstream_url: env::var("UPSTREAM_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_UPSTREAM_URL.to_string()),
            upstream_timeout: Duration::from_millis(parse_u64(
                "UPSTREAM_TIMEOUT_MS",
                10_000,
                1,
                u64::MAX,
            )?),
            cache_ttl: Duration::from_millis(parse_u64("CACHE_TTL_MS", 300_000, 0, u64::MAX)?),
            cache_max_stale: Duration::from_millis(parse_u64(
                "CACHE_MAX_STALE_MS",
                86_400_000,
                0,
                u64::MAX,
            )?),
            default_count: parse_u64("DEFAULT_COUNT", 10, 1, u64::MAX)? as usize,
            max_count: parse_u64("MAX_COUNT", 100, 1, u64::MAX)? as usize,
        })
    }
}

fn parse_u64(name: &str, default: u64, min: u64, max: u64) -> Result<u64, String> {
    match env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => {
            let value: u64 = raw
                .trim()
                .parse()
                .map_err(|_| format!("{name} must be an integer, got \"{raw}\""))?;
            if value < min || value > max {
                return Err(format!("{name} must be between {min} and {max}, got {value}"));
            }
            Ok(value)
        }
        _ => Ok(default),
    }
}
