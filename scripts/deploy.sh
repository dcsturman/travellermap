#!/usr/bin/env bash
#
# Ship to Cloud Run. Builds the image in Cloud Build (amd64, matching Cloud Run)
# and deploys it. This is the one command to run on every push to production.
#
# Config comes from deploy.env (copy deploy.env.example). One-time setup —
# enabling APIs, creating the Artifact Registry repo, and mapping the custom
# domain — is documented in DEPLOY.md (it's done once, so it's not scripted here).
#
set -euo pipefail
# This script lives in scripts/; config sits beside it, but the Cloud Build
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

# Build via Kaniko (cloudbuild.yaml) so layers are cached in Artifact Registry
# across builds — the first run populates the cache (full time); later runs reuse
# the base image / toolchain / crate-fetch / compile layers and are much faster.
echo ">> building + pushing $IMAGE via Cloud Build (Kaniko, cached)…"
gcloud builds submit \
  --project "$PROJECT_ID" \
  --config cloudbuild.yaml \
  --substitutions=_IMAGE="$IMAGE" \
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
