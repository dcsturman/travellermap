# Traveller Map — Rust rewrite

This directory is the Rust/Leptos rewrite of Traveller Map (see the repo-root
[`CLAUDE.md`](../CLAUDE.md) "Mission" and [`PORT_PLAN.md`](../PORT_PLAN.md) for the
why and the roadmap). It's a Cargo workspace; the upstream C#/JS reference tree
and the shared `res/` data live alongside it at the repo root.

## Crates

| Crate | What it is | Runs where |
| --- | --- | --- |
| **`core`** (`tmap-core`) | Pure shared domain — astrometrics, DTOs, world/UWP parsing. **No I/O**, so it compiles for both native and wasm. | native + wasm |
| **`backend`** (`tmap-backend`) | axum server. Streams sector/metadata JSON, serves `res/` assets, and (in a deployed image) serves the built frontend. **No image rendering.** | native, port **3000** (local) / `$PORT` |
| **`frontend`** (`tmap-frontend`) | Leptos → WASM client. Renders the map in the browser. | wasm, Trunk dev server port **8080** |

> Rendering happens **in the browser** — the backend's only job is to stream data.
> The frontend calls the API with **relative `/api/...` URLs**, so in production a
> single container serves both from one origin (see [Deploy](#deploy)).

## Prerequisites

```sh
# Rust (stable) + the wasm target for the frontend
rustup target add wasm32-unknown-unknown

# Trunk drives the wasm build / dev server
cargo install trunk --locked        # or: cargo binstall trunk

# For the container build / deploy only:
#   - Docker (local image)            https://docs.docker.com/get-docker/
#   - gcloud CLI (Cloud Run deploy)   https://cloud.google.com/sdk/docs/install
```

## Run locally (dev — two servers)

Run from the **repo root** (the backend reads `res/` relative to its working dir).

```sh
# Terminal 1 — data API on http://127.0.0.1:3000
cargo run -p tmap-backend

# Terminal 2 — wasm app on http://127.0.0.1:8080 (proxies /api → :3000)
cd crates/frontend
trunk serve                          # plain map
trunk serve --features callisto      # + worldgen popups (double-click a system,
                                     #   "World Map" button) via the external
                                     #   Callisto service — no local worldgen needed
```

Open **http://127.0.0.1:8080**. Trunk live-reloads the frontend on save; restart
the backend to pick up backend changes.

## Build & test

```sh
# Native (core + backend). The wasm-only frontend is excluded from default-members.
cargo build
cargo test                # backend incl. the public-API compatibility suite

# Frontend: type-check against wasm, or produce a release bundle in dist/
cargo check -p tmap-frontend --target wasm32-unknown-unknown
cd crates/frontend && trunk build --release --features callisto
```

## Environment variables

| Var | Used by | Default | Meaning |
| --- | --- | --- | --- |
| `PORT` | backend | `3000` | Listen port (Cloud Run injects this; binds `0.0.0.0`). |
| `TMAP_RES_DIR` | backend | `res` | Path to the shared `res/` data tree. |
| `TMAP_DIST_DIR` | backend | `dist` | Built frontend bundle to serve; if the dir is absent, static serving is skipped (the dev case — Trunk serves the frontend). |
| `TMAP_ENABLE_ADMIN` | backend | _(unset)_ | Truthy (`1`/`true`/`yes`/`on`) mounts `POST /api/admin/flush` (a dev/profiling cache-flush). Unset → the route 404s, so production never exposes it. |

## Run the production image locally (one origin)

The single-container build (frontend + API on one port) — what actually deploys.
Build/deploy scripts live in [`../scripts/`](../scripts/); the `Dockerfile` is at
the repo root. Run from the repo root:

```sh
scripts/build.sh run        # builds the image, runs it on http://localhost:8080
```

## Deploy

Cloud Run, scale-to-zero. Full walk-through (one-time GCP setup, custom-domain
mapping) is in [`../DEPLOY.md`](../DEPLOY.md). Once that's done, shipping is one
command — from the **repo root**:

```sh
cp scripts/deploy.env.example scripts/deploy.env   # fill in PROJECT_ID, …  (one-time)
scripts/deploy.sh                                  # Cloud Build (amd64) → Cloud Run
```
