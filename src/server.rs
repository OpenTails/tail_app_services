use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::cache::{PostCache, ProductCache};
use crate::config::Config;
use crate::rng::Rng;
use crate::select::select_products;
use crate::upstream::{fetch_posts, fetch_products};

/// Shared state for every request.
pub struct App {
    pub cache: ProductCache,
    pub blog_cache: PostCache,
    pub agent: ureq::Agent,
    pub config: Config,
}

struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

struct Response {
    status: u16,
    x_cache: Option<&'static str>,
    body: Option<Value>,
}

/// Accept connections forever, handling each one on its own thread.
pub fn run(listener: TcpListener, app: Arc<App>) -> io::Result<()> {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let app = Arc::clone(&app);
                thread::spawn(move || {
                    if let Err(err) = handle(stream, &app) {
                        let error: &dyn std::error::Error = &err;
                        tracing::error!(error, "connection error");
                    }
                });
            }
            Err(err) => tracing::warn!(error = %err, "accept error"),
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream, app: &App) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    let Some(request) = read_request(&mut stream)? else {
        return Ok(());
    };

    let (path, query) = split_target(&request.target);
    let sentry_trace = request.header("sentry-trace").unwrap_or_default();
    let span = tracing::info_span!(
        "http.request",
        sentry.name = %format!("{} {}", request.method, path),
        sentry.op = "http.server",
        sentry.trace = sentry_trace,
        http.method = %request.method,
        http.target = %path,
        http.response.status_code = tracing::field::Empty,
        cache.source = tracing::field::Empty,
    );
    let _entered = span.enter();

    attach_request_context(&request, path, query);

    let started = Instant::now();
    let response = route(app, &request);
    span.record("http.response.status_code", response.status as i64);
    let result = write_response(&mut stream, &request.method, &response);
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

    sentry::metrics::counter("http.requests", 1.0)
        .attribute("http.method", request.method.clone())
        .attribute("http.route", path.to_string())
        .attribute("http.response.status_code", response.status as i64)
        .capture();
    sentry::metrics::distribution("http.request.duration", elapsed_ms)
        .unit(sentry::protocol::Unit::Millisecond)
        .attribute("http.method", request.method.clone())
        .attribute("http.route", path.to_string())
        .capture();

    result
}

fn route(app: &App, request: &Request) -> Response {
    if request.method == "OPTIONS" {
        return Response {
            status: 204,
            x_cache: None,
            body: None,
        };
    }
    if request.method != "GET" && request.method != "HEAD" {
        return Response::error(405, "method_not_allowed");
    }

    let (path, query) = split_target(&request.target);
    match path {
        "/health" => Response::json(200, json!({ "status": "ok" })),
        "/" => Response::json(
            200,
            json!({
                "name": "tailcompany-wp-proxy",
                "endpoints": {
                    "GET /products": "Random products from the Store API, with on-sale items first.",
                    "GET /blog": "The 10 latest blog posts.",
                    "GET /health": "Liveness check.",
                },
                "query": {
                    "count": format!(
                        "Products to return, 1-{} (default {}).",
                        app.config.max_count,
                        app.config.default_count.min(app.config.max_count)
                    ),
                },
            }),
        ),
        "/products" => products(app, query),
        "/blog" => posts(app),
        _ => Response::error(404, "not_found"),
    }
}

fn products(app: &App, query: &str) -> Response {
    let count = match parse_count(query, &app.config) {
        Ok(count) => count,
        Err(message) => {
            return Response::json(400, json!({ "error": "bad_request", "message": message }));
        }
    };

    match app
        .cache
        .get_or_load(|| fetch_products(&app.agent, &app.config))
    {
        Ok((products, source)) => {
            tracing::Span::current().record("cache.source", source.as_str());
            sentry::configure_scope(|scope| scope.set_tag("cache.source", source.as_str()));
            tracing::info!(
                cache.source = source.as_str(),
                cache.products = products.len(),
                "cache lookup"
            );

            sentry::metrics::counter("cache.requests", 1.0)
                .attribute("cache.source", source.as_str())
                .attribute("http.route", "/products")
                .capture();
            sentry::metrics::gauge("cache.products", products.len() as f64).capture();

            let selection = select_products(&products, count, &mut Rng::from_entropy());
            Response {
                status: 200,
                x_cache: Some(source.as_str()),
                body: Some(json!(selection)),
            }
        }
        Err(err) => {
            let error: &dyn std::error::Error = &err;
            tracing::error!(error, "upstream request failed");
            Response::json(502, json!({ "error": "upstream_unavailable" }))
        }
    }
}

fn posts(app: &App) -> Response {
    match app
        .blog_cache
        .get_or_load(|| fetch_posts(&app.agent, &app.config))
    {
        Ok((posts, source)) => {
            tracing::Span::current().record("cache.source", source.as_str());
            sentry::configure_scope(|scope| scope.set_tag("cache.source", source.as_str()));
            tracing::info!(
                cache.source = source.as_str(),
                cache.posts = posts.len(),
                "cache lookup"
            );

            sentry::metrics::counter("cache.requests", 1.0)
                .attribute("cache.source", source.as_str())
                .attribute("http.route", "/blog")
                .capture();
            sentry::metrics::gauge("cache.posts", posts.len() as f64).capture();

            Response {
                status: 200,
                x_cache: Some(source.as_str()),
                body: Some(json!(posts)),
            }
        }
        Err(err) => {
            let error: &dyn std::error::Error = &err;
            tracing::error!(error, "upstream request failed");
            Response::json(502, json!({ "error": "upstream_unavailable" }))
        }
    }
}

fn parse_count(query: &str, config: &Config) -> Result<usize, String> {
    for pair in query.split('&') {
        let Some((key, raw)) = pair.split_once('=') else {
            continue;
        };
        if key != "count" || raw.is_empty() {
            continue;
        }
        let value: usize = raw
            .parse()
            .map_err(|_| "\"count\" must be a positive integer".to_string())?;
        if value == 0 {
            return Err("\"count\" must be a positive integer".to_string());
        }
        return Ok(value.min(config.max_count));
    }
    Ok(config.default_count.min(config.max_count))
}

fn split_target(target: &str) -> (&str, &str) {
    match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    }
}

impl Response {
    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            x_cache: None,
            body: Some(body),
        }
    }

    fn error(status: u16, code: &str) -> Self {
        Self::json(status, json!({ "error": code }))
    }
}

fn read_request(stream: &mut TcpStream) -> io::Result<Option<Request>> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 64 * 1024 {
            break;
        }
    }

    if buffer.is_empty() {
        return Ok(None);
    }

    let text = String::from_utf8_lossy(&buffer);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_uppercase();
    let target = parts.next().unwrap_or("/").to_string();

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }

    Ok(Some(Request {
        method,
        target,
        headers,
    }))
}

/// Attach HTTP request data and tags to the current Sentry scope so error
/// events carry the request that produced them.
fn attach_request_context(request: &Request, path: &str, query: &str) {
    let host = request.header("host").unwrap_or("localhost");
    let mut context = sentry::protocol::Request {
        url: format!("http://{host}{}", request.target).parse().ok(),
        method: Some(request.method.clone()),
        query_string: if query.is_empty() {
            None
        } else {
            Some(query.to_string())
        },
        ..Default::default()
    };
    for name in ["host", "user-agent", "accept", "content-type"] {
        if let Some(value) = request.header(name) {
            context.headers.insert(name.to_string(), value.to_string());
        }
    }

    sentry::configure_scope(|scope| {
        scope.set_tag("http.method", &request.method);
        scope.set_tag("http.route", path);
        if let Some(span) = scope.get_span() {
            span.set_request(context);
        }
    });
}

fn write_response(stream: &mut TcpStream, method: &str, response: &Response) -> io::Result<()> {
    let body = match &response.body {
        Some(value) => serde_json::to_vec(value).unwrap_or_default(),
        None => Vec::new(),
    };

    let mut head = String::new();
    head.push_str(&format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    ));
    head.push_str("content-type: application/json; charset=utf-8\r\n");
    head.push_str("cache-control: no-store\r\n");
    if let Some(cache) = response.x_cache {
        head.push_str(&format!("x-cache: {cache}\r\n"));
    }
    head.push_str("access-control-allow-origin: *\r\n");
    head.push_str("access-control-allow-methods: GET, HEAD, OPTIONS\r\n");
    head.push_str("access-control-allow-headers: content-type\r\n");
    head.push_str("connection: close\r\n");
    head.push_str(&format!("content-length: {}\r\n\r\n", body.len()));

    stream.write_all(head.as_bytes())?;
    if method != "HEAD" && !body.is_empty() {
        stream.write_all(&body)?;
    }
    stream.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "OK",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            port: 0,
            upstream_url: "http://unused.invalid".to_string(),
            blog_upstream_url: "http://unused.invalid".to_string(),
            upstream_timeout: Duration::from_secs(1),
            cache_ttl: Duration::from_secs(3600),
            cache_max_stale: Duration::from_secs(3600),
            default_count: 10,
            max_count: 100,
            sentry_dsn: None,
            sentry_environment: "test".to_string(),
            sentry_release: "test".to_string(),
            sentry_traces_sample_rate: 0.0,
            sentry_sample_rate: 1.0,
            sentry_debug: false,
        }
    }

    /// Build an app whose cache is already warm, so tests never touch the
    /// network.
    fn test_app(config: Config) -> Arc<App> {
        let cache = ProductCache::new(config.cache_ttl, config.cache_max_stale);
        let products: Vec<crate::product::Product> = (1..=25)
            .map(|id| crate::product::Product {
                id,
                name: format!("Product {id}"),
                on_sale: id % 5 == 0,
                is_in_stock: true,
                permalink: format!("https://example.test/product/{id}/"),
                thumbnail: Some(format!("https://example.test/{id}.jpg")),
            })
            .collect();
        cache.get_or_load(|| Ok(products)).unwrap();

        let blog_cache = PostCache::new(config.cache_ttl, config.cache_max_stale);
        let posts: Vec<crate::post::Post> = (1..=10)
            .map(|id| crate::post::Post {
                id,
                name: format!("Post {id}"),
                date: format!("2026-01-{id:02}T00:00:00"),
                url: format!("https://example.test/2026/01/post-{id}/"),
                thumbnail: Some(format!("https://example.test/post-{id}.jpg")),
            })
            .collect();
        blog_cache.get_or_load(|| Ok(posts)).unwrap();

        Arc::new(App {
            cache,
            blog_cache,
            agent: ureq::Agent::new_with_defaults(),
            config,
        })
    }

    fn spawn(config: Config) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let app = test_app(config);
        thread::spawn(move || {
            let _ = run(listener, app);
        });
        format!("http://{address}")
    }

    fn get(url: &str) -> (u16, Option<String>, String) {
        let mut response = ureq::get(url).call().unwrap_or_else(|err| panic!("{err}"));
        let status = response.status().as_u16();
        let cache = response
            .headers()
            .get("x-cache")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response.body_mut().read_to_string().unwrap();
        (status, cache, body)
    }

    #[test]
    fn products_returns_ten_products_with_only_the_exposed_fields() {
        let base = spawn(test_config());
        let (status, cache, body) = get(&format!("{base}/products"));

        assert_eq!(status, 200);
        assert_eq!(cache.as_deref(), Some("cache"));

        let items: Vec<Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(items.len(), 10);
        for item in &items {
            let mut keys: Vec<&str> = item
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                vec![
                    "id",
                    "is_in_stock",
                    "name",
                    "on_sale",
                    "permalink",
                    "thumbnail"
                ]
            );
        }
    }

    #[test]
    fn products_puts_on_sale_items_first() {
        let base = spawn(test_config());
        let (_, _, body) = get(&format!("{base}/products?count=8"));

        let items: Vec<Value> = serde_json::from_str(&body).unwrap();
        let flags: Vec<bool> = items
            .iter()
            .map(|item| item["on_sale"].as_bool().unwrap())
            .collect();
        assert_eq!(
            flags,
            vec![true, true, true, true, true, false, false, false]
        );
    }

    #[test]
    fn products_honors_and_validates_count() {
        let base = spawn(test_config());

        let (_, _, body) = get(&format!("{base}/products?count=3"));
        assert_eq!(
            serde_json::from_str::<Value>(&body)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            3
        );

        let request = ureq::get(format!("{base}/products?count=0")).call();
        let status = match request {
            Ok(response) => response.status().as_u16(),
            Err(ureq::Error::StatusCode(code)) => code,
            Err(err) => panic!("{err}"),
        };
        assert_eq!(status, 400);
    }

    #[test]
    fn blog_returns_ten_posts_with_only_the_exposed_fields() {
        let base = spawn(test_config());
        let (status, cache, body) = get(&format!("{base}/blog"));

        assert_eq!(status, 200);
        assert_eq!(cache.as_deref(), Some("cache"));

        let items: Vec<Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(items.len(), 10);
        for item in &items {
            let mut keys: Vec<&str> = item
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(keys, vec!["date", "id", "name", "thumbnail", "url"]);
        }
    }

    #[test]
    fn serves_health_root_and_unknown_routes() {
        let base = spawn(test_config());

        assert_eq!(get(&format!("{base}/health")).0, 200);
        assert_eq!(get(&format!("{base}/")).0, 200);

        let missing = ureq::get(format!("{base}/nope")).call();
        let status = match missing {
            Ok(response) => response.status().as_u16(),
            Err(ureq::Error::StatusCode(code)) => code,
            Err(err) => panic!("{err}"),
        };
        assert_eq!(status, 404);
    }
}
