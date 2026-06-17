#!/usr/bin/env bash
#
# Ship to Cloud Run. Builds the image LOCALLY (docker buildx, linux/amd64 to
# match Cloud Run) with BuildKit cache mounts for fast incremental rebuilds,
# pushes it to Artifact Registry, and deploys. The one command to run per push.
#
# Requires a running local Docker (Docker Desktop). Config comes from deploy.env
# (copy deploy.env.example). One-time setup — enabling APIs, creating the
# Artifact Registry repo, and mapping the custom domain — is in DEPLOY.md.
#
set -euo pipefail
# This script lives in scripts/; config sits beside it, but the docker build
# context is the repo root — so resolve both and cd to the root.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

# --- config ---
if [[ -f "$SCRIPT_DIR/deploy.env" ]]; then set -a; source "$SCRIPT_DIR/deploy.env"; set +a; fi
: "${PROJECT_ID:?set PROJECT_ID in scripts/deploy.env}"
: "${REGION:=us-central1}"
: "${SERVICE:=travellermap}"
: "${REPO:=travellermap}"
: "${MEMORY:=1Gi}"
: "${CPU:=1}"
: "${MAX_INSTANCES:=4}"

# Tag the image with the current commit for traceability (falls back to "latest"
# outside a git checkout).
TAG="$(git rev-parse --short HEAD 2>/dev/null || echo latest)"
IMAGE="${REGION}-docker.pkg.dev/${PROJECT_ID}/${REPO}/${SERVICE}:${TAG}"

# Ensure the Artifact Registry repo exists (idempotent — folds the one-time setup
# into the deploy so a fresh project just works; no-op once it's there).
if ! gcloud artifacts repositories describe "$REPO" \
      --project "$PROJECT_ID" --location "$REGION" >/dev/null 2>&1; then
  echo ">> Artifact Registry repo '$REPO' not found in $REGION — creating it…"
  gcloud services enable artifactregistry.googleapis.com --project "$PROJECT_ID"
  gcloud artifacts repositories create "$REPO" \
    --project "$PROJECT_ID" --location "$REGION" \
    --repository-format=docker --description="Traveller Map images"
fi

# Build LOCALLY with buildx + BuildKit cache mounts (see Dockerfile). Unlike the
# old Kaniko-in-Cloud-Build path — which recompiled the whole workspace on every
# push because any source change busts the `COPY . . && cargo build` layer — the
# target/ cache mount persists cargo's incremental compile cache in the local
# BuildKit daemon, so a code change recompiles only what changed. Cloud Run is
# amd64-only, so we build linux/amd64 (emulated on Apple Silicon; still fast once
# the cache is warm, as the recompile is incremental).
#
# Both setup steps are idempotent: configure Docker to push to Artifact Registry,
# and ensure a buildx builder on the docker-container driver (the default 'docker'
# driver supports neither cache export nor persistent cache mounts).
gcloud auth configure-docker "${REGION}-docker.pkg.dev" --quiet
docker buildx inspect tmap-builder >/dev/null 2>&1 \
  || docker buildx create --name tmap-builder --driver docker-container --use
docker buildx use tmap-builder

# The registry cache (--cache-to/from) carries LAYERS across machines/fresh
# checkouts; the big incremental win (the target/ mount) lives in the local
# daemon and persists automatically between local builds.
BUILDCACHE="${REGION}-docker.pkg.dev/${PROJECT_ID}/${REPO}/${SERVICE}:buildcache"
echo ">> building + pushing $IMAGE locally via buildx (linux/amd64, cached)…"
docker buildx build \
  --platform linux/amd64 \
  --cache-from=type=registry,ref="$BUILDCACHE" \
  --cache-to=type=registry,ref="$BUILDCACHE",mode=max \
  -t "$IMAGE" \
  --push \
  .

echo ">> deploying $SERVICE to Cloud Run ($REGION)…"
gcloud run deploy "$SERVICE" \
  --project "$PROJECT_ID" \
  --region "$REGION" \
  --image "$IMAGE" \
  --platform managed \
  --allow-unauthenticated \
  --memory "$MEMORY" \
  --cpu "$CPU" \
  --min-instances 0 \
  --max-instances "$MAX_INSTANCES"

echo ">> deployed. Service URL:"
gcloud run services describe "$SERVICE" \
  --project "$PROJECT_ID" --region "$REGION" \
  --format='value(status.url)'
