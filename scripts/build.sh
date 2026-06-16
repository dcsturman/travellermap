#!/usr/bin/env bash
#
# Build the deployable image LOCALLY and (optionally) run it, so you can verify
# the whole app — Leptos/WASM frontend + data API on one origin — before
# shipping. This builds for your machine's architecture; the production image is
# built for amd64 in Cloud Build by deploy.sh.
#
#   scripts/build.sh        # build the image
#   scripts/build.sh run    # build, then run on http://localhost:8080
#
set -euo pipefail
# Build context is the repo root (Dockerfile + crates/ + res/ live there); this
# script sits in scripts/, so step up one level.
cd "$(dirname "$0")/.."

IMAGE="${IMAGE:-tmap-local}"

echo ">> building $IMAGE (compiles the wasm frontend + release backend — minutes)…"
docker build -t "$IMAGE" .

if [[ "${1:-}" == "run" ]]; then
  echo ">> running $IMAGE on http://localhost:8080  (Ctrl-C to stop)"
  # --init runs tini as PID 1 so Ctrl-C (SIGINT) actually stops the container —
  # otherwise the backend is PID 1, which ignores default-action signals, and the
  # container keeps running after the foreground `docker run` exits.
  exec docker run --rm --init -e PORT=8080 -p 8080:8080 "$IMAGE"
fi

echo ">> built $IMAGE.  Try it:  docker run --rm -e PORT=8080 -p 8080:8080 $IMAGE"
