use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::post::Post;
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
struct Inflight<T> {
    result: Mutex<Option<Result<Vec<T>, String>>>,
    ready: Condvar,
}

impl<T> Default for Inflight<T> {
    fn default() -> Self {
        Self {
            result: Mutex::new(None),
            ready: Condvar::new(),
        }
    }
}

struct State<T> {
    items: Option<Vec<T>>,
    fetched_at: Option<Instant>,
    inflight: Option<Arc<Inflight<T>>>,
}

/// A TTL cache for one kind of upstream list.
///
/// - Fresh entries are served without calling the loader.
/// - Concurrent misses are coalesced into a single loader call.
/// - If the loader fails, the last good list is served while it is younger
///   than `max_stale`.
pub struct Cache<T> {
    state: Mutex<State<T>>,
    ttl: Duration,
    max_stale: Duration,
}

/// The cache holding the upstream product list.
pub type ProductCache = Cache<Product>;

/// The cache holding the upstream blog post list.
pub type PostCache = Cache<Post>;

impl<T: Clone> Cache<T> {
    pub fn new(ttl: Duration, max_stale: Duration) -> Self {
        Self {
            state: Mutex::new(State {
                items: None,
                fetched_at: None,
                inflight: None,
            }),
            ttl,
            max_stale,
        }
    }

    pub fn get_or_load<F>(&self, loader: F) -> Result<(Vec<T>, CacheSource), UpstreamError>
    where
        F: FnOnce() -> Result<Vec<T>, UpstreamError>,
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

        let had_entry = state.items.is_some();
        let inflight = Arc::new(Inflight::default());
        state.inflight = Some(Arc::clone(&inflight));
        drop(state);

        let result = loader().map_err(|err| err.0);

        {
            let mut state = self.state.lock().unwrap();
            state.inflight = None;
            if let Ok(items) = &result {
                state.items = Some(items.clone());
                state.fetched_at = Some(Instant::now());
            }
        }

        Self::publish(&inflight, result.clone());

        match result {
            Ok(items) => Ok((
                items,
                if had_entry {
                    CacheSource::Refresh
                } else {
                    CacheSource::Upstream
                },
            )),
            Err(message) => self.stale_or_err(message),
        }
    }

    fn fresh(&self, state: &State<T>) -> Option<Vec<T>> {
        match (&state.items, state.fetched_at) {
            (Some(items), Some(fetched_at)) if fetched_at.elapsed() < self.ttl => {
                Some(items.clone())
            }
            _ => None,
        }
    }

    fn stale_or_err(&self, message: String) -> Result<(Vec<T>, CacheSource), UpstreamError> {
        let state = self.state.lock().unwrap();
        if let (Some(items), Some(fetched_at)) = (&state.items, state.fetched_at)
            && fetched_at.elapsed() < self.max_stale
        {
            return Ok((items.clone(), CacheSource::Stale));
        }
        Err(UpstreamError(message))
    }

    fn finish(
        &self,
        result: Result<Vec<T>, String>,
    ) -> Result<(Vec<T>, CacheSource), UpstreamError> {
        match result {
            // The shared load refreshed the cache, so it is fresh now.
            Ok(items) => Ok((items, CacheSource::Cache)),
            Err(message) => self.stale_or_err(message),
        }
    }

    fn wait(inflight: &Inflight<T>) -> Result<Vec<T>, String> {
        let mut guard = inflight.result.lock().unwrap();
        while guard.is_none() {
            guard = inflight.ready.wait(guard).unwrap();
        }
        guard.clone().expect("inflight result is set before notify")
    }

    fn publish(inflight: &Inflight<T>, result: Result<Vec<T>, String>) {
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
            thumbhash: None,
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
