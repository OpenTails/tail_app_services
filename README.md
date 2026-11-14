# tailcompany-wp-proxy

A small Rust proxy in front of the Tail Company WooCommerce Store API. It
compiles to a single self-contained binary, caches the upstream list in memory,
and reports errors and performance traces to Sentry.

- Upstream: `https://thetailcompany.com/wp-json/wc/store/v1/products?tag=select&per_page=100`
- Endpoint: `GET /products` → JSON array of 10 products (configurable)
- On-sale products are always listed first
- Only the fields needed by a storefront widget are exposed
- Sentry error reporting + tracing, and a distroless Docker image

## Requirements

A Rust toolchain to build/test. The runtime needs nothing beyond the binary.

## Build

```bash
cargo build --release
# binary: target/release/tailcompany-wp-proxy  (~2.2 MB, stripped)
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
    "thumbnail": "https://thetailcompany.com/wp-content/uploads/2026/04/VS1-300x300.jpg"
  }
]
```

`GET /products?count=5` returns a different number of products (clamped to
`MAX_COUNT`). The `x-cache` response header reports `upstream`, `cache`,
`refresh`, or `stale`. Other routes: `GET /` (self-description) and `GET /health`
(liveness).

## How it works

```
GET /products ──▶ TTL cache ──(miss)──▶ Store API ──▶ normalize ──▶ select ──▶ JSON
                     │(hit)
                     └────────────────────────────────────────────▶ select ──▶ JSON
```

1. **Caching** (`src/cache.rs`) — the full upstream list is kept in memory for
   `CACHE_TTL_MS`. Concurrent misses are coalesced into a single upstream
   request, and if upstream fails the last good copy is served (up to
   `CACHE_MAX_STALE_MS`) instead of erroring.
2. **Normalizing** (`src/product.rs`) — each product is projected down to
   `id`, `name`, `on_sale`, `is_in_stock`, `permalink`, and the first image's
   `thumbnail` (falling back to its full `src`, then `null`). Malformed entries
   are skipped rather than failing the whole batch.
3. **Selecting** (`src/select.rs`) — products are split into on-sale and regular
   groups, each group is shuffled (Fisher-Yates), and the groups are concatenated
   on-sale first. The result is truncated to the requested count, so on-sale
   products always occupy the leading positions and regular products only make up
   the balance when there are not enough sale items. Results are unique and
   freshly randomized per request.
4. **Telemetry** (`src/telemetry.rs`) — Sentry is initialised at startup and a
   `tracing` subscriber fans events out to the console and to Sentry.

## Configuration

All settings are environment variables. A `.env` file in the working directory
is loaded at startup; real environment variables take precedence.

| Variable | Default | Description |
| --- | --- | --- |
| `PORT` | `3000` | Listen port. The server binds `0.0.0.0`. |
| `UPSTREAM_URL` | the Store API URL above | Override the source URL. |
| `UPSTREAM_TIMEOUT_MS` | `10000` | Per-request upstream timeout. |
| `CACHE_TTL_MS` | `300000` | How long a cached list stays fresh (5 min). |
| `CACHE_MAX_STALE_MS` | `86400000` | How long an expired list may be served when upstream is down (24 h). |
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
  cache behavior, and upstream fetches: `http.requests`,
  `http.request.duration`, `cache.requests`, `cache.products`,
  `upstream.fetch.duration`, `upstream.products`, and `upstream.errors`.
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
46 MB.

```bash
docker build -t tailcompany-wp-proxy .
docker run --rm -p 3000:3000 --env-file .env tailcompany-wp-proxy
```

`.env` is not baked into the image; pass it at runtime with `--env-file` (or
`-e SENTRY_DSN=...`).

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
| `src/cache.rs` | TTL cache with request coalescing and stale-if-error fallback. |
| `src/upstream.rs` | Fetches the Store API over HTTPS (ureq + rustls). |
| `src/product.rs` | Lenient normalization down to the six exposed fields. |
| `src/select.rs` | On-sale-first random selection (Fisher-Yates per group). |
| `src/rng.rs` | Small SplitMix64 generator, so there is no `rand` dependency. |

## Dependencies

Kept deliberately small: `ureq` (blocking HTTP client with rustls), `serde`,
`serde_json`, `sentry` (using its `ureq` transport, so no reqwest/tokio), and
`tracing`/`tracing-subscriber`. The HTTP server is written directly against
`std::net`, so there is no async runtime.

## Tests

```bash
cargo test
```

Twenty unit and integration tests cover normalization, on-sale-first selection,
the cache (fresh hits, TTL refresh, coalescing, stale fallback), and the HTTP
layer. The HTTP tests warm the cache up front, so they never touch the network.

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
