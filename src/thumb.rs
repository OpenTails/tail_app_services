use std::collections::HashMap;
use std::sync::Mutex;
use std::thread;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// Number of images fetched and hashed concurrently per refresh.
const WORKERS: usize = 8;

/// Upper bound on remembered hashes before the map is reset, so a long-running
/// process whose image URLs keep changing stays bounded.
const MAX_ENTRIES: usize = 4096;

/// A process-wide cache of ThumbHashes keyed by image URL.
///
/// A cache refresh reuses the hash of any image whose URL is unchanged, so a
/// hash is only computed when its URL is new. Only successes are remembered: a
/// transient failure is simply retried on the next refresh.
#[derive(Default)]
pub struct ThumbhashCache {
    entries: Mutex<HashMap<String, String>>,
}

impl ThumbhashCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a base64 ThumbHash for each image URL, preserving input order.
    ///
    /// URLs already remembered are reused; the rest are fetched and hashed a
    /// few at a time. A `None` URL, a network error, or an unsupported/corrupt
    /// image yields `None` for that entry; the batch itself always succeeds.
    pub fn resolve(&self, agent: &ureq::Agent, urls: &[Option<String>]) -> Vec<Option<String>> {
        let requested = urls.iter().filter(|url| url.is_some()).count();
        let mut results: Vec<Option<String>> = vec![None; urls.len()];
        let mut missing: Vec<(usize, String)> = Vec::new();
        let mut reused = 0usize;

        {
            let entries = self.entries.lock().unwrap();
            for (index, url) in urls.iter().enumerate() {
                let Some(url) = url else { continue };
                match entries.get(url) {
                    Some(hash) => {
                        results[index] = Some(hash.clone());
                        reused += 1;
                    }
                    None => missing.push((index, url.clone())),
                }
            }
        }

        let mut computed = 0usize;
        if !missing.is_empty() {
            let missing_urls: Vec<String> = missing.iter().map(|(_, url)| url.clone()).collect();
            let hashes = compute_many(agent, &missing_urls);

            let mut entries = self.entries.lock().unwrap();
            if entries.len() + hashes.len() > MAX_ENTRIES {
                entries.clear();
            }
            for ((index, url), hash) in missing.into_iter().zip(hashes) {
                if let Some(hash) = hash {
                    entries.insert(url, hash.clone());
                    results[index] = Some(hash);
                    computed += 1;
                }
            }
        }

        sentry::metrics::gauge("thumbhash.computed", computed as f64).capture();
        sentry::metrics::gauge("thumbhash.reused", reused as f64).capture();
        sentry::metrics::counter("thumbhash.failed", (requested - computed - reused) as f64)
            .capture();

        results
    }
}

/// Fetch and hash a batch of URLs, spread across a few threads.
fn compute_many(agent: &ureq::Agent, urls: &[String]) -> Vec<Option<String>> {
    if urls.is_empty() {
        return Vec::new();
    }

    let workers = WORKERS.min(urls.len()).max(1);
    let chunk = urls.len().div_ceil(workers);
    let mut out = Vec::with_capacity(urls.len());

    thread::scope(|scope| {
        let handles: Vec<(usize, _)> = urls
            .chunks(chunk)
            .map(|slice| {
                let len = slice.len();
                let handle = scope.spawn(move || {
                    slice
                        .iter()
                        .map(|url| compute(agent, url))
                        .collect::<Vec<_>>()
                });
                (len, handle)
            })
            .collect();

        for (len, handle) in handles {
            match handle.join() {
                Ok(hashes) => out.extend(hashes),
                Err(_) => out.extend(std::iter::repeat_n(None, len)),
            }
        }
    });

    out
}

/// Fetch `url` and encode it as a base64 ThumbHash, or `None` on any failure.
fn compute(agent: &ureq::Agent, url: &str) -> Option<String> {
    let mut response = agent.get(url).call().ok()?;
    let bytes = response.body_mut().read_to_vec().ok()?;
    from_bytes(&bytes)
}

/// Encode already-downloaded image bytes (JPEG/PNG/WebP/GIF) as a base64
/// ThumbHash.
///
/// The decoded image is downscaled to fit within 100x100, the largest size the
/// ThumbHash format supports, while keeping its aspect ratio.
fn from_bytes(bytes: &[u8]) -> Option<String> {
    let image = image::load_from_memory(bytes).ok()?;
    let rgba = image.thumbnail(100, 100).to_rgba8();
    let (width, height) = rgba.dimensions();
    let hash = thumbhash::rgba_to_thumb_hash(width as usize, height as usize, rgba.as_raw());
    Some(STANDARD.encode(hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba, RgbaImage};
    use std::io::Cursor;

    fn encoded_png(width: u32, height: u32) -> Vec<u8> {
        let image = RgbaImage::from_pixel(width, height, Rgba([200, 40, 40, 255]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn encodes_a_base64_thumbhash() {
        let hash = from_bytes(&encoded_png(64, 32)).expect("png should decode");
        assert!(!hash.is_empty());
        // Standard base64, so it only uses the base64 alphabet.
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c))
        );
        // The thumbhash of a 64x32 image should be a similar byte length.
        let raw = STANDARD.decode(&hash).unwrap();
        assert!(raw.len() >= 20 && raw.len() <= 40, "raw len: {}", raw.len());
    }

    #[test]
    fn round_trips_through_the_decoder() {
        let hash = from_bytes(&encoded_png(64, 32)).expect("png should decode");
        let raw = STANDARD.decode(&hash).unwrap();
        let Ok((width, height, rgba)) = thumbhash::thumb_hash_to_rgba(&raw) else {
            panic!("valid thumbhash should decode");
        };
        assert!(width > 0 && height > 0);
        assert_eq!(rgba.len(), width * height * 4);
    }

    #[test]
    fn downscales_oversized_images() {
        // Larger than the format's 100px limit; must still encode.
        assert!(from_bytes(&encoded_png(320, 240)).is_some());
    }

    #[test]
    fn returns_none_for_invalid_bytes() {
        assert!(from_bytes(b"not an image").is_none());
    }

    #[test]
    fn empty_input_yields_no_hashes() {
        let cache = ThumbhashCache::new();
        let agent = ureq::Agent::new_with_defaults();
        assert!(cache.resolve(&agent, &[]).is_empty());
    }

    #[test]
    fn reuses_cached_hashes_when_the_url_is_unchanged() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let png = encoded_png(32, 32);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));

        let counter = Arc::clone(&requests);
        let body = png.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                counter.fetch_add(1, Ordering::SeqCst);
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: image/png\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });

        let agent = ureq::Agent::new_with_defaults();
        let cache = ThumbhashCache::new();
        let urls = vec![Some(format!("http://{address}/a.png"))];

        let first = cache.resolve(&agent, &urls);
        let second = cache.resolve(&agent, &urls);

        assert!(first[0].is_some());
        assert_eq!(first, second);
        assert_eq!(
            requests.load(Ordering::SeqCst),
            1,
            "the image should only be fetched once"
        );
    }
}
