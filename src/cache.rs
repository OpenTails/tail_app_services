use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::product::Product;
use crate::upstream::UpstreamError;

/// Where a result came from, surfaced through the `x-cache` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheSource {
    Upstream,
    Refresh,
    Cache,
    Stale,
}

impl CacheSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheSource::Upstream => "upstream",
            CacheSource::Refresh => "refresh",
            CacheSource::Cache => "cache",
            CacheSource::Stale => "stale",
        }
    }
}

/// A one-shot load shared by every request that missed the cache at the same
/// time, so the upstream is only hit once per refresh.
#[derive(Default)]
struct Inflight {
    result: Mutex<Option<Result<Vec<Product>, String>>>,
    ready: Condvar,
}

struct State {
    products: Option<Vec<Product>>,
    fetched_at: Option<Instant>,
    inflight: Option<Arc<Inflight>>,
}

/// A TTL cache for the upstream product list.
///
/// - Fresh entries are served without calling the loader.
/// - Concurrent misses are coalesced into a single loader call.
/// - If the loader fails, the last good list is served while it is younger
///   than `max_stale`.
pub struct ProductCache {
    state: Mutex<State>,
    ttl: Duration,
    max_stale: Duration,
}

impl ProductCache {
    pub fn new(ttl: Duration, max_stale: Duration) -> Self {
        Self {
            state: Mutex::new(State {
                products: None,
                fetched_at: None,
                inflight: None,
            }),
            ttl,
            max_stale,
        }
    }

    pub fn get_or_load<F>(&self, loader: F) -> Result<(Vec<Product>, CacheSource), UpstreamError>
    where
        F: FnOnce() -> Result<Vec<Product>, UpstreamError>,
    {
        let mut state = self.state.lock().unwrap();

        if let Some(fresh) = self.fresh(&state) {
            return Ok((fresh, CacheSource::Cache));
        }

        // Someone else is already loading: wait for their result.
        if let Some(inflight) = state.inflight.clone() {
            drop(state);
            let result = Self::wait(&inflight);
            return self.finish(result);
        }

        let had_entry = state.products.is_some();
        let inflight = Arc::new(Inflight::default());
        state.inflight = Some(Arc::clone(&inflight));
        drop(state);

        let result = loader().map_err(|err| err.0);

        {
            let mut state = self.state.lock().unwrap();
            state.inflight = None;
            if let Ok(products) = &result {
                state.products = Some(products.clone());
                state.fetched_at = Some(Instant::now());
            }
        }

        Self::publish(&inflight, result.clone());

        match result {
            Ok(products) => Ok((
                products,
                if had_entry {
                    CacheSource::Refresh
                } else {
                    CacheSource::Upstream
                },
            )),
            Err(message) => self.stale_or_err(message),
        }
    }

    fn fresh(&self, state: &State) -> Option<Vec<Product>> {
        match (&state.products, state.fetched_at) {
            (Some(products), Some(fetched_at)) if fetched_at.elapsed() < self.ttl => {
                Some(products.clone())
            }
            _ => None,
        }
    }

    fn stale_or_err(&self, message: String) -> Result<(Vec<Product>, CacheSource), UpstreamError> {
        let state = self.state.lock().unwrap();
        if let (Some(products), Some(fetched_at)) = (&state.products, state.fetched_at)
            && fetched_at.elapsed() < self.max_stale
        {
            return Ok((products.clone(), CacheSource::Stale));
        }
        Err(UpstreamError(message))
    }

    fn finish(
        &self,
        result: Result<Vec<Product>, String>,
    ) -> Result<(Vec<Product>, CacheSource), UpstreamError> {
        match result {
            // The shared load refreshed the cache, so it is fresh now.
            Ok(products) => Ok((products, CacheSource::Cache)),
            Err(message) => self.stale_or_err(message),
        }
    }

    fn wait(inflight: &Inflight) -> Result<Vec<Product>, String> {
        let mut guard = inflight.result.lock().unwrap();
        while guard.is_none() {
            guard = inflight.ready.wait(guard).unwrap();
        }
        guard.clone().expect("inflight result is set before notify")
    }

    fn publish(inflight: &Inflight, result: Result<Vec<Product>, String>) {
        let mut guard = inflight.result.lock().unwrap();
        *guard = Some(result);
        drop(guard);
        inflight.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    fn sample() -> Vec<Product> {
        vec![Product {
            id: 1,
            name: "A".to_string(),
            on_sale: false,
            is_in_stock: true,
            permalink: "p".to_string(),
            thumbnail: None,
        }]
    }

    #[test]
    fn serves_a_fresh_value_from_cache() {
        let cache = ProductCache::new(Duration::from_secs(60), Duration::from_secs(60));
        let calls = AtomicUsize::new(0);

        let first = cache.get_or_load(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(sample())
        });
        let second = cache.get_or_load(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(sample())
        });

        assert_eq!(first.unwrap().1, CacheSource::Upstream);
        assert_eq!(second.unwrap().1, CacheSource::Cache);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn refreshes_after_the_ttl_expires() {
        let cache = ProductCache::new(Duration::from_millis(20), Duration::from_secs(60));
        cache.get_or_load(|| Ok(sample())).unwrap();
        thread::sleep(Duration::from_millis(40));

        let refreshed = cache.get_or_load(|| Ok(sample())).unwrap();
        assert_eq!(refreshed.1, CacheSource::Refresh);
    }

    #[test]
    fn serves_stale_when_the_loader_fails() {
        let cache = ProductCache::new(Duration::from_millis(20), Duration::from_secs(60));
        cache.get_or_load(|| Ok(sample())).unwrap();
        thread::sleep(Duration::from_millis(40));

        let stale = cache
            .get_or_load(|| Err(UpstreamError("down".to_string())))
            .unwrap();
        assert_eq!(stale.1, CacheSource::Stale);
        assert_eq!(stale.0.len(), 1);
    }

    #[test]
    fn errors_when_there_is_nothing_to_fall_back_to() {
        let cache = ProductCache::new(Duration::from_secs(60), Duration::from_secs(60));
        assert!(
            cache
                .get_or_load(|| Err(UpstreamError("boom".to_string())))
                .is_err()
        );
    }

    #[test]
    fn stops_serving_stale_after_max_stale() {
        let cache = ProductCache::new(Duration::from_millis(10), Duration::from_millis(30));
        cache.get_or_load(|| Ok(sample())).unwrap();
        thread::sleep(Duration::from_millis(60));

        assert!(
            cache
                .get_or_load(|| Err(UpstreamError("gone".to_string())))
                .is_err()
        );
    }

    #[test]
    fn coalesces_concurrent_loads() {
        let cache = Arc::new(ProductCache::new(
            Duration::from_secs(60),
            Duration::from_secs(60),
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(3));

        let handles: Vec<_> = (0..3)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let calls = Arc::clone(&calls);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    cache
                        .get_or_load(|| {
                            calls.fetch_add(1, Ordering::SeqCst);
                            thread::sleep(Duration::from_millis(40));
                            Ok(sample())
                        })
                        .unwrap()
                })
            })
            .collect();

        for handle in handles {
            assert_eq!(handle.join().unwrap().0.len(), 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
