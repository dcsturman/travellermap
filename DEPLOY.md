# Deploying (Cloud Run)

The Rust rewrite ships as **one container**: the axum backend serves the data API
**and** the compiled Leptos/WASM frontend from a single origin, so the whole app
lives at one URL — e.g. `https://travellermap.callistoflight.com`. It scales to
zero on Cloud Run (no database; the dataset loads into RAM from the bundled
`res/`), which suits low, bursty traffic.

## Layout

| File | Role |
| --- | --- |
| `Dockerfile` | Multi-stage build: Trunk builds the wasm frontend, cargo builds the backend, runtime image = binary + `dist/` + `res/`. |
| `.dockerignore` | Trims the build context to the Cargo manifests, `crates/`, and `res/`. |
| `build.sh` | Build the image **locally** and run it on `:8080` to verify before shipping. |
| `deploy.sh` | **Per-push:** build in Cloud Build (amd64) + deploy to Cloud Run. |
| `deploy.env.example` | Copy to `deploy.env` (gitignored) and fill in project/region/etc. |

> Standing rule: anything done more than once is a script. `build.sh` / `deploy.sh`
> are the recurring paths; only the genuinely one-time setup below is manual.

## Verify locally first

```sh
cp deploy.env.example deploy.env   # then edit it
./build.sh run                     # builds the image, runs it on http://localhost:8080
```

Open <http://localhost:8080> — the map, the API (`/api/...`), the `res/` assets,
and the worldgen popups should all work from that one port, exactly as they will
in production. (`PORT` is read from the environment; Cloud Run injects it.)

## One-time GCP setup

Done once per project — not scripted because it never repeats.

```sh
# Fill these to match deploy.env.
PROJECT_ID=your-gcp-project-id
REGION=us-central1
REPO=travellermap

gcloud config set project "$PROJECT_ID"

# Enable the APIs deploy.sh uses.
gcloud services enable run.googleapis.com artifactregistry.googleapis.com cloudbuild.googleapis.com

# Artifact Registry repo that deploy.sh pushes images to.
gcloud artifacts repositories create "$REPO" \
  --repository-format=docker --location="$REGION" \
  --description="Traveller Map images"
```

## Ship

```sh
./deploy.sh
```

Builds the image in Cloud Build (so the architecture matches Cloud Run and no
local Docker is needed), pushes it, deploys, and prints the service URL. Run it
on every push to production.

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

*Optional, later:* front it with an external HTTPS load balancer + Cloud CDN to
cache the WASM bundle and `res/` assets at the edge (matches the "CDN-cacheable
static" design in CLAUDE.md). Not needed for v1.

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
