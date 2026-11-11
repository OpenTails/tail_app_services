# tailcompany-wp-proxy

A small Rust proxy in front of the Tail Company WooCommerce Store API. It
compiles to a single self-contained binary and provides an in-memory TTL cache,
so upstream is not hit on every request.

- Upstream: `https://thetailcompany.com/wp-json/wc/store/v1/products?tag=select&per_page=100`
- Endpoint: `GET /products` → JSON array of 10 products (configurable)
- On-sale products are always listed first
- Only the fields needed by a storefront widget are exposed

## Requirements

A Rust toolchain (build/test). The runtime needs nothing beyond the binary.

## Build

```bash
cargo build --release
# binary: target/release/tailcompany-wp-proxy  (~2.2 MB, stripped)
```

## Run

```bash
PORT=3000 ./target/release/tailcompany-wp-proxy
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

## Configuration

All settings are environment variables.

| Variable | Default | Description |
| --- | --- | --- |
| `PORT` | `3000` | Listen port. The server binds `0.0.0.0`. |
| `UPSTREAM_URL` | the Store API URL above | Override the source URL. |
| `UPSTREAM_TIMEOUT_MS` | `10000` | Per-request upstream timeout. |
| `CACHE_TTL_MS` | `300000` | How long a cached list stays fresh (5 min). |
| `CACHE_MAX_STALE_MS` | `86400000` | How long an expired list may be served when upstream is down (24 h). |
| `DEFAULT_COUNT` | `10` | Products returned when `?count=` is omitted. |
| `MAX_COUNT` | `100` | Upper bound for `?count=`. |

```bash
CACHE_TTL_MS=60000 PORT=8080 ./target/release/tailcompany-wp-proxy
```

## Layout

| File | Role |
| --- | --- |
| `src/main.rs` | Reads config, builds the agent/app, binds and serves. |
| `src/config.rs` | Environment configuration. |
| `src/server.rs` | Hand-rolled HTTP/1.1 server (thread per connection), routing, JSON responses. |
| `src/cache.rs` | TTL cache with request coalescing and stale-if-error fallback. |
| `src/upstream.rs` | Fetches the Store API over HTTPS (ureq + rustls). |
| `src/product.rs` | Lenient normalization down to the six exposed fields. |
| `src/select.rs` | On-sale-first random selection (Fisher-Yates per group). |
| `src/rng.rs` | Small SplitMix64 generator, so there is no `rand` dependency. |

## Dependencies

Kept deliberately small: `ureq` (blocking HTTP client with rustls), `serde` and
`serde_json`. The HTTP server is written directly against `std::net`, so there is
no async runtime.

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
