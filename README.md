# tailcompany-wp-proxy

A small Rust proxy in front of the Tail Company WooCommerce Store API. It
compiles to a single self-contained binary, caches the upstream list in memory,
and reports errors and performance traces to Sentry.

- Upstream: `https://thetailcompany.com/wp-json/wc/store/v1/products?tag=select&per_page=100`
- Sale feed: `https://thetailcompany.com/wp-json/tailco/v1/sale-items`
- Endpoint: `GET /products` → JSON array of 10 products (configurable)
- On-sale products are always listed first
- Endpoint: `GET /blog` → JSON array of the 10 latest blog posts
- Every image is accompanied by a base64 ThumbHash placeholder
- Only the fields needed by a storefront widget are exposed
- Sentry error reporting + tracing, and a distroless Docker image

## Requirements

A Rust toolchain to build/test. The runtime needs nothing beyond the binary.

## Build

```bash
cargo build --release
# binary: target/release/tailcompany-wp-proxy  (~6.3 MB)
```

Or build the container:

```bash
docker build -t tailcompany-wp-proxy .
```

## Run

```bash
PORT=3000 ./target/release/tailcompany-wp-proxy
```

```bash
docker run --rm -p 3000:3000 tailcompany-wp-proxy
```

```bash
curl http://localhost:3000/products
```

```json
[
  {
    "id": 78013,
    "name": "Sway Wings",
    "on_sale": false,
    "is_in_stock": true,
    "permalink": "https://thetailcompany.com/product/sway-wings/",
    "thumbnail": "https://thetailcompany.com/wp-content/uploads/2026/04/VS1-300x300.jpg",
    "thumbhash": "KAgGDwKtp4j5UWhMw3fCmqdpRwenVXAK"
  }
]
```

`GET /products?count=5` returns a different number of products (clamped to
`MAX_COUNT`). The `x-cache` response header reports `upstream`, `cache`,
`refresh`, or `stale`. Other routes: `GET /` (self-description) and `GET /health`
(liveness).

```bash
curl http://localhost:3000/blog
```

```json
[
  {
    "id": 81281,
    "name": "A day in the life..",
    "date": "2026-10-29T08:41:24",
    "url": "https://thetailcompany.com/2026/10/29/a-day-in-the-life/",
    "thumbnail": "https://thetailcompany.com/wp-content/uploads/2026/10/custom-mitail-280x280.jpg",
    "thumbhash": "ZggOBwCQmHl7loh4hZisRZeKB9iff6IE"
  }
]
```

`GET /blog` reads the WordPress REST API (`wp/v2/posts`) and always returns the
10 latest posts, newest first, as configured by `BLOG_UPSTREAM_URL`. Titles have
their HTML entities decoded (e.g. `&#8217;` → `’`).

Both endpoints add a `thumbhash` field: a base64 [ThumbHash](https://github.com/evanw/thumbhash)
placeholder that clients can decode to an instant blurred preview while the full
image loads. It is `null` when the item has no image or the image could not be
fetched or decoded. See [ThumbHash generation](#thumbhash-generation) for the
cost this adds to a cache refresh.

## How it works

```
GET /products ──▶ TTL cache ──(miss)──▶ Store API ─┐
                     │                             ├─▶ mark on-sale ──▶ select ──▶ JSON
                     │                 sale-items ─┘
                     │(hit)
                     └────────────────────────────────────────────▶ select ──▶ JSON

GET /blog ──────▶ TTL cache ──(miss)──▶ Posts API ──▶ normalize ────────────▶ JSON
                     │(hit)
                     └──────────────────────────────────────────────────────▶ JSON
```

1. **Caching** (`src/cache.rs`) — the full upstream list is kept in memory for
   `CACHE_TTL_MS`. Concurrent misses are coalesced into a single upstream
   request, and if upstream fails the last good copy is served (up to
   `CACHE_MAX_STALE_MS`) instead of erroring.
2. **Normalizing** (`src/product.rs`, `src/post.rs`) — each product is projected
   down to `id`, `name`, `on_sale`, `is_in_stock`, `permalink`, and the first
   image's `thumbnail` (falling back to its full `src`, then `null`); each post
   is projected down to `id`, `name` (with HTML entities decoded), `date`,
   `url`, and the featured image's `thumbnail` size (falling back to `medium`,
   `large`, `full`, then the full-size `source_url`). Malformed entries are
   skipped rather than failing the whole batch. Each product is then marked
   `on_sale` by matching its id against the `tailco/v1/sale-items` endpoint
   (`src/sale.rs`); the Store API's own `on_sale` field is ignored. A failure of
   the sale feed is logged but does not fail the product list — products are
   served without a sale badge instead.
3. **ThumbHashing** (`src/thumb.rs`) — each item's image is fetched, decoded
   (JPEG/PNG/WebP/GIF), downscaled to fit 100×100, and encoded as a ThumbHash,
   then base64-encoded into the `thumbhash` field. Images are fetched a few at a
   time; a failure just leaves that item's `thumbhash` as `null`.
4. **Selecting** (`src/select.rs`) — products are split into on-sale and regular
   groups, each group is shuffled (Fisher-Yates), and the groups are concatenated
   on-sale first. The result is truncated to the requested count, so on-sale
   products always occupy the leading positions and regular products only make up
   the balance when there are not enough sale items. Results are unique and
   freshly randomized per request. Blog posts keep the upstream order and are
   served as-is (newest first).
5. **Telemetry** (`src/telemetry.rs`) — Sentry is initialised at startup and a
   `tracing` subscriber fans events out to the console and to Sentry.

### ThumbHash generation

ThumbHashes are computed during a cache refresh, not per request, so cache hits
stay sub-millisecond. They are memoized in memory by image URL: a refresh only
downloads and hashes images whose URL is new or changed, and unchanged images
are reused from the memo. Only successes are remembered, so a transient failure
is retried on the next refresh.

The upstream does not provide hashes, so the first time an image is seen it is
downloaded as part of the refresh:

- `/blog` hashes its ~10 images (roughly 1–2 s on a cold refresh).
- `/products` caches the full upstream list (up to `per_page`, 100 by default)
  and hashes all of it, so any later random selection is covered (roughly 3 s on
  a cold refresh).

Only the request that performs a cold refresh pays this cost; concurrent misses
are coalesced onto it, and later refreshes reuse the memo. Set
`THUMBHASH_ENABLED=false` to skip it entirely (the field is still present but
always `null`).

## Configuration

All settings are environment variables. A `.env` file in the working directory
is loaded at startup; real environment variables take precedence.

| Variable | Default | Description |
| --- | --- | --- |
| `PORT` | `3000` | Listen port. The server binds `0.0.0.0`. |
| `UPSTREAM_URL` | the Store API URL above | Override the source URL. |
| `SALE_ITEMS_URL` | the sale-items URL above | Override the sale-items source URL that decides which products are on sale. |
| `BLOG_UPSTREAM_URL` | the posts API URL above | Override the blog source URL (also controls how many posts are returned). |
| `UPSTREAM_TIMEOUT_MS` | `10000` | Per-request upstream timeout. |
| `CACHE_TTL_MS` | `300000` | How long a cached list stays fresh (5 min). |
| `CACHE_MAX_STALE_MS` | `86400000` | How long an expired list may be served when upstream is down (24 h). |
| `THUMBHASH_ENABLED` | `true` | Generate base64 ThumbHash placeholders. Adds one image download per item to each cache refresh. |
| `DEFAULT_COUNT` | `10` | Products returned when `?count=` is omitted. |
| `MAX_COUNT` | `100` | Upper bound for `?count=`. |
| `SENTRY_DSN` | unset (Sentry disabled) | Sentry DSN, usually set in `.env`. |
| `SENTRY_ENVIRONMENT` | `production` | Sentry environment tag. |
| `SENTRY_RELEASE` | `tailcompany-wp-proxy@<version>` | Sentry release name. |
| `SENTRY_SAMPLE_RATE` | `1.0` | Fraction of error events sent, `0.0`–`1.0`. |
| `SENTRY_TRACES_SAMPLE_RATE` | `1.0` | Fraction of requests traced, `0.0`–`1.0` (`/health` is never traced). |
| `SENTRY_DEBUG` | `false` | Log Sentry transport activity to stderr. |
| `RUST_LOG` | `info` | Console log filter (e.g. `debug`, `tailcompany_wp_proxy=debug`). |

```bash
CACHE_TTL_MS=60000 PORT=8080 ./target/release/tailcompany-wp-proxy
```

## Observability (Sentry)

Sentry is enabled when `SENTRY_DSN` is set. The proxy loads a `.env` file at
startup, so the usual setup is:

```bash
cp .env.example .env
# edit .env and set SENTRY_DSN
```

Leave `SENTRY_DSN` empty to disable Sentry.

What is captured:

- **Errors** — `ERROR`-level `tracing` events become Sentry error events with
  exception type and source chain. This covers upstream failures and connection
  errors. Panics are captured by the `panic` integration. Error events carry the
  request context (URL, method, query string, and safe headers) plus
  `http.method` / `http.route` / `cache.source` tags. When the upstream returns
  malformed JSON, a truncated copy of the response is attached to the event.
- **Logs** — `INFO`-level and above `tracing` events are also sent as Sentry
  structured logs (via `LogLayer`).
- **Metrics** — counters, gauges, and distributions are captured for requests,
  cache behavior, upstream fetches, and placeholders: `http.requests`,
  `http.request.duration`, `cache.requests`, `cache.products`, `cache.posts`,
  `upstream.fetch.duration`, `upstream.products`, `upstream.posts`,
  `upstream.errors`, and the ThumbHash counters `thumbhash.computed` /
  `thumbhash.reused` / `thumbhash.failed`.
- **Tracing** — each HTTP request is a Sentry transaction (`http.server`, named
  e.g. `GET /products`), and the upstream fetch is a child span. Spans carry
  `http.response.status_code`, `cache.source`, and `upstream.url`. Incoming
  `sentry-trace` headers are continued, and the trace is propagated to the
  upstream request. Sampling is controlled by `SENTRY_TRACES_SAMPLE_RATE`
  (default `1.0`); `/health` is never traced.
- **Breadcrumbs** — `INFO`/`WARN` events, including cache lookups, are attached
  as breadcrumbs to later errors.

Release builds keep the symbol table (`strip = "debuginfo"`) so Sentry
backtraces resolve to function names. Line numbers require debug info; upload
debug files to Sentry (or build with `debug = 1` and no stripping) if you need
them.

The DSN is a public client key, not a server secret; it can be overridden per
environment. Set `SENTRY_DEBUG=true` to see the transport's activity.

## Docker

The `Dockerfile` is a multi-stage build: `rust:1-slim-bookworm` compiles the
binary, then it is copied into `gcr.io/distroless/cc-debian12:nonroot`. The
runtime image has no shell or package manager, runs as UID 65532, and is about
50 MB.

```bash
docker build -t tailcompany-wp-proxy .
docker run --rm -p 3000:3000 --env-file .env tailcompany-wp-proxy
```

`.env` is not baked into the image; pass it at runtime with `--env-file` (or
`-e SENTRY_DSN=...`).

Or run it with Compose (`docker-compose.yml`, a single `proxy` service):

```bash
cp .env.example .env        # optional; set SENTRY_DSN and any overrides
docker compose up --build
```

Compose reads `.env` automatically and forwards the proxy's settings to the
container; `PORT` drives both the published and the container port. The
distroless runtime has no shell or HTTP client, so there is no container
healthcheck — probe `GET /health` from outside.

CI (`.github/workflows/docker.yml`) builds the image on every push and pull
request, and publishes it to `ghcr.io/opentails/tail_app_services` on pushes to
`master` and `v*` tags. Dependabot (`.github/dependabot.yml`) keeps Cargo,
GitHub Actions, and Docker base images up to date.

## Layout

| File | Role |
| --- | --- |
| `src/main.rs` | Reads config, initialises telemetry, builds the app, binds and serves. |
| `src/config.rs` | Environment configuration. |
| `src/telemetry.rs` | Sentry + `tracing` subscriber setup. |
| `src/server.rs` | Hand-rolled HTTP/1.1 server (thread per connection), routing, JSON responses. |
| `src/cache.rs` | Generic TTL cache with request coalescing and stale-if-error fallback. |
| `src/upstream.rs` | Fetches the Store, sale-items, and posts APIs over HTTPS (ureq + rustls). |
| `src/product.rs` | Lenient normalization down to the seven exposed fields, marking on-sale ids. |
| `src/sale.rs` | Parses the sale-items endpoint into the set of on-sale product ids. |
| `src/post.rs` | Lenient normalization down to the six exposed post fields. |
| `src/thumb.rs` | Downloads, downscales, and ThumbHashes images, memoized by URL. |
| `src/select.rs` | On-sale-first random selection (Fisher-Yates per group). |
| `src/rng.rs` | Small SplitMix64 generator, so there is no `rand` dependency. |

## Dependencies

Kept deliberately small: `ureq` (blocking HTTP client with rustls), `serde`,
`serde_json`, `sentry` (using its `ureq` transport, so no reqwest/tokio), and
`tracing`/`tracing-subscriber`. `image` (JPEG/PNG/WebP/GIF decoding),
`thumbhash`, and `base64` are used to produce the placeholder hashes; `base64`
was already present via `ureq`. The HTTP server is written directly against
`std::net`, so there is no async runtime.

## Tests

```bash
cargo test
```

Thirty-eight unit and integration tests cover product, sale-item, and post
normalization, ThumbHash encoding and reuse, on-sale-first selection, the cache
(fresh hits, TTL refresh, coalescing, stale fallback), and the HTTP layer. The
HTTP tests warm the cache up front, so they never touch the network.

CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `cargo clippy`, and
`cargo test` on every push and pull request.

## Static binary (optional)

The default build links against the system C library. For a fully statically
linked binary, install the musl target and build against it:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

## Notes

- The proxy is read-only and safe to put behind a CDN or reverse proxy.
- CORS is open (`Access-Control-Allow-Origin: *`) so browser clients can call it
  directly. Tighten the headers in `src/server.rs` if you need to lock it down.
- The cache is per-process and in-memory; running multiple instances means each
  keeps its own copy.

## License

GPL-3.0-only. See [LICENSE](LICENSE).
