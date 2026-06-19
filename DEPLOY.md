# Deploying (Cloud Run)

The Rust rewrite ships as **one container**: the axum backend serves the data API
**and** the compiled Leptos/WASM frontend from a single origin, so the whole app
lives at one URL — e.g. `https://travellermap.callistoflight.com`. It scales to
zero on Cloud Run (no database; the dataset loads into RAM from the bundled
`res/`), which suits low, bursty traffic.

## Layout

| File | Role |
| --- | --- |
| `Dockerfile` | Multi-stage build: Trunk builds the wasm frontend, cargo builds the backend, runtime image = binary + `dist/` + `res/`. Uses BuildKit cache mounts (`target/` + cargo registry) for fast incremental rebuilds — build it with `docker buildx`, not Kaniko. |
| `.dockerignore` | Trims the build context to the Cargo manifests, `crates/`, and `res/`. |
| `scripts/build.sh` | Build the image **locally** and run it on `:8080` to verify before shipping. |
| `scripts/deploy.sh` | **Per-push:** build locally with `docker buildx` (linux/amd64, cached) + push + deploy to Cloud Run. Auto-creates the Artifact Registry repo if missing. |
| `scripts/deploy.env.example` | Copy to `scripts/deploy.env` (gitignored) and fill in project/region/etc. |

> Standing rule: anything done more than once is a script. `scripts/build.sh` /
> `scripts/deploy.sh` are the recurring paths; only the genuinely one-time setup
> below is manual. See [`scripts/README.md`](scripts/README.md) for the config vars.

## Verify locally first

```sh
cp scripts/deploy.env.example scripts/deploy.env   # then edit it
scripts/build.sh run               # builds the image, runs it on http://localhost:8080
```

Open <http://localhost:8080> — the map, the API (`/api/...`), the `res/` assets,
and the worldgen popups should all work from that one port, exactly as they will
in production. (`PORT` is read from the environment; Cloud Run injects it.)

## One-time GCP setup

```sh
# Fill these to match scripts/deploy.env.
PROJECT_ID=your-gcp-project-id

gcloud config set project "$PROJECT_ID"

# Enable the APIs the deploy uses. (deploy.sh also auto-enables Artifact Registry
# and creates the repo on first run, so this is mostly belt-and-suspenders.)
# Cloud Build is no longer used — the image is built locally with docker buildx.
gcloud services enable run.googleapis.com artifactregistry.googleapis.com
```

The Artifact Registry repo no longer needs to be created by hand — `deploy.sh`
creates it (idempotently) if it's missing.

## Ship

```sh
scripts/deploy.sh
```

Builds the image locally with `docker buildx` (linux/amd64, matching Cloud Run),
pushes it to Artifact Registry, deploys, and prints the service URL. Requires a
running local Docker (Docker Desktop). Run it on every push to production.

### Build speed / caching

The build uses **BuildKit cache mounts** (in the `Dockerfile`): cargo's crate
registry and the `target/` incremental-compile cache persist in the local
BuildKit daemon across builds, so a code change recompiles **only what changed**
— dependencies are compiled once and never again. The **first** build is slow
(cold cache, and on Apple Silicon the amd64 toolchain runs under QEMU emulation);
**subsequent** builds are fast incremental recompiles.

`deploy.sh` also pushes a registry layer-cache (`<service>:buildcache`) via
`--cache-to/--cache-from`, which lets a *fresh* machine reuse layers — but the big
incremental win is the local cache mounts, which the registry cache can't carry.
The buildx builder (`tmap-builder`, docker-container driver) is created
idempotently on first run.

## Map the custom domain (one-time)

```sh
SERVICE=travellermap
REGION=us-central1
DOMAIN=travellermap.callistoflight.com

gcloud beta run domain-mappings create \
  --service "$SERVICE" --domain "$DOMAIN" --region "$REGION"
```

It prints DNS records (a `CNAME`, or `A`/`AAAA`) to add at your DNS provider for
`travellermap.callistoflight.com`. Add them; Google provisions a managed TLS
certificate automatically (can take a few minutes to an hour to go live).

## CDN (Cloudflare)

Front the origin with Cloudflare so the hot sectors (and the WASM bundle + `res/`
assets) are served from the edge under load instead of cold-starting a container.
The data API is **static between deploys** (no datastore; data changes only on an
upstream pull + redeploy), so it edge-caches cleanly — the trade-off is handled by
purging the edge on every deploy (below) rather than by short TTLs.

**How the caching works.** The data endpoints (`/api/sector/*`, `/api/universe`,
`/api/overlays`, `/api/metadata`, `/data/*`, …) send
`Cache-Control: public, max-age=300, s-maxage=86400, stale-while-revalidate=86400`
plus a content-hash `ETag` (see `serve_cached` in `crates/backend/src/main.rs`).
A browser holds a copy for 5 min then revalidates with a cheap 304; Cloudflare's
shared cache (`s-maxage`) serves it from the edge for a day. `/api/res/*` static
assets send `max-age=86400`. **The catch:** Cloudflare does **not** cache `/api/*`
JSON by default (its default cache only triggers on static file extensions), so a
Cache Rule is required (step 3) to make it honour those headers.

DNS for `callistoflight.com` is already on Cloudflare, so this is three dashboard
steps + the purge token:

1. **Proxy the record.** DNS › the `travellermap` record (the `CNAME` →
   `ghs.googlehosted.com` from the domain mapping above) → set to **Proxied**
   (orange cloud). Keep the Cloud Run domain mapping in place — Google keeps a
   managed cert for `travellermap.callistoflight.com`, which is what makes step 2
   validate.
2. **SSL/TLS mode › Full (strict).** Cloudflare terminates TLS at the edge and
   re-encrypts to the origin, validating Google's managed cert for the custom
   domain. (Flexible/Off would break or downgrade; Full-without-strict skips
   validation — use strict.)
3. **Cache Rule** (Caching › Cache Rules › Create): match
   `(http.request.uri.path starts_with "/api/" or http.request.uri.path starts_with "/data/")`,
   action **Eligible for cache**, Edge TTL **"Use cache-control header if present,
   bypass cache if not."** This makes Cloudflare honour the origin headers above —
   so `serve_cached` responses cache, while anything that sends `no-cache` (e.g.
   `/api/search`) is left uncached. (The WASM bundle and `res/` already cache via
   their extensions/headers, no rule needed.)

**Purge on deploy.** Because the edge holds data for a day, `scripts/deploy.sh`
flushes it after every deploy via `scripts/purge-cdn.sh` so new data is live
immediately. Configure two values in `scripts/deploy.env` (gitignored):

- `CF_ZONE_ID` — Cloudflare dashboard › the zone's **Overview** page (right column).
- `CF_API_TOKEN` — My Profile › API Tokens › Create Token, scoped to
  **Zone › Cache Purge › Purge** for the `callistoflight.com` zone (nothing more).

With those set the deploy ends with `>> CDN cache purged.`; unset, it prints
`CDN purge skipped` and the deploy still succeeds. Purge manually any time with
`scripts/purge-cdn.sh`.

**Verify** a sector is being edge-served after a deploy:

```sh
curl -sI 'https://travellermap.callistoflight.com/api/sector/M1105/Spinward%20Marches' \
  | grep -i 'cf-cache-status\|cache-control'
# First hit: cf-cache-status: MISS (or EXPIRED); a second hit: HIT.
```

## Notes

- **Cold starts:** with scale-to-zero, the first request after an idle period
  boots a container. The M1105 universe parse is ~150 ms, and the backend warms
  it in the background at startup, so cold-start data latency is negligible — the
  container serves the static `index.html`/WASM immediately while the cache fills.
  If you ever want zero cold starts, add `--min-instances 1` (trades the
  scale-to-zero savings for an always-warm instance).
- **Admin routes:** `POST /api/admin/flush` (a dev/profiling cache-flush) is
  **not mounted** unless `TMAP_ENABLE_ADMIN` is truthy (`1`/`true`/`yes`/`on`).
  Leave it unset in production (it 404s); set it locally only when profiling.
- **CORS:** the API keeps permissive CORS — it's a public, read-only data API
  meant to be callable cross-origin by third-party tools (the frontend itself is
  same-origin and needs none).
- **`callisto` feature:** the shipped frontend is built with `--features callisto`
  so the worldgen solar-system / world-map popups are live; they call the external
  Callisto service and pull no worldgen code into this build.
