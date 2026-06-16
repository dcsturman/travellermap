# scripts/

Build & deploy scripts for the Rust rewrite. Run them from anywhere — each
`cd`s to the repo root itself (the Docker / Cloud Build context). The full
deployment walk-through (one-time GCP setup, custom-domain mapping) is in
[`../DEPLOY.md`](../DEPLOY.md).

| File | What it does |
| --- | --- |
| `build.sh` | Build the deployable image **locally** and optionally run it. `scripts/build.sh run` serves the whole app (frontend + API, one origin) on http://localhost:8080 — the smoke test before shipping. Builds for your machine's arch. |
| `deploy.sh` | **Ship to Cloud Run.** Builds the image in Cloud Build (amd64, via `../cloudbuild.yaml` → Kaniko with layer caching) and deploys it. Auto-creates the Artifact Registry repo if missing. The one command to run per push. |
| `deploy.env.example` | Template for `deploy.env` (gitignored) — the project/region/sizing config `deploy.sh` reads. |

## Setup (once)

```sh
cp scripts/deploy.env.example scripts/deploy.env   # then edit it
```

## Configuration (`scripts/deploy.env`)

Sourced by `deploy.sh`; any value can also be overridden by exporting it in the
environment.

| Var | Required | Default | Meaning |
| --- | --- | --- | --- |
| `PROJECT_ID` | ✅ | — | GCP project ID. |
| `REGION` | | `us-central1` | Cloud Run + Artifact Registry region. |
| `SERVICE` | | `travellermap` | Cloud Run service name. |
| `REPO` | | `travellermap` | Artifact Registry (docker) repo name. |
| `DOMAIN` | | `travellermap.callistoflight.com` | Custom domain (mapped once; see `../DEPLOY.md`). |
| `MEMORY` | | `1Gi` | Instance memory (the dataset lives in RAM). |
| `CPU` | | `1` | Instance vCPUs. |
| `MAX_INSTANCES` | | `4` | Max Cloud Run instances (min is 0 → scales to zero). |

## Typical use

```sh
scripts/build.sh run    # verify locally on :8080
scripts/deploy.sh       # Cloud Build → Cloud Run
```

The container's own runtime env vars (`PORT`, `TMAP_RES_DIR`, `TMAP_DIST_DIR`,
`TMAP_ENABLE_ADMIN`) are documented in [`../crates/README.md`](../crates/README.md).
