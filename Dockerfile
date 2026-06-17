# syntax=docker/dockerfile:1
#
# Single-container build for the Rust rewrite: the axum backend serves BOTH the
# data API and the compiled Leptos/WASM frontend from one origin, so the whole
# app lives at a single URL (ideal for Cloud Run). See DEPLOY.md.

# ---- Build stage: compile the wasm frontend (Trunk) + the native backend ----
FROM rust:1-bookworm AS builder

# wasm target for the Leptos frontend.
RUN rustup target add wasm32-unknown-unknown

# Trunk drives the wasm build (it auto-fetches a matching wasm-bindgen + wasm-opt
# on first build). Grab the prebuilt binary for the *target* architecture. BuildKit
# auto-populates $TARGETARCH (arm64 on Apple-silicon local builds); the default
# below covers builders that DON'T set it — notably Cloud Build's legacy Docker
# engine, which is amd64 anyway. The `trunk --version` check ensures the binary
# actually runs on this arch — otherwise fall back to compiling from source.
ARG TARGETARCH=amd64
ARG TRUNK_VERSION=0.21.4
RUN set -eux; \
    case "${TARGETARCH:-amd64}" in \
      amd64) tarch=x86_64 ;; \
      arm64) tarch=aarch64 ;; \
      *)     tarch=x86_64 ;; \
    esac; \
    ( curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-${tarch}-unknown-linux-gnu.tar.gz" \
        | tar -xz -C /usr/local/bin trunk && trunk --version ) \
    || cargo install trunk --version "${TRUNK_VERSION}" --locked

WORKDIR /src
COPY . .

# Frontend → crates/frontend/dist. `--features callisto` enables the worldgen
# solar-system / world-map popups (they call the external Callisto HTTP service;
# no worldgen crate is pulled into this build).
#
# The cache mounts persist cargo's crate registry + the target/ incremental
# compile cache in the BuildKit daemon across builds, so a code change recompiles
# only what changed (deps are built once, never again). dist/ lives outside
# target/, so the wasm assets still commit to this layer normally. NOTE: this
# requires building with BuildKit/buildx — Kaniko ignores cache mounts (see
# scripts/deploy.sh, which builds locally via `docker buildx`).
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,id=tmap-target-${TARGETARCH},target=/src/target \
    cd crates/frontend && trunk build --release --features callisto

# Native backend (release profile: LTO, opt-level 3 — see root Cargo.toml). The
# binary lands INSIDE the target/ cache mount, which is NOT part of the committed
# layer, so copy it out to /out before the RUN ends — the runtime stage COPYs
# from there. (Same shared cache mounts as the wasm step above.)
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,id=tmap-target-${TARGETARCH},target=/src/target \
    cargo build --release -p tmap-backend && \
    mkdir -p /out && cp target/release/tmap-backend /out/tmap-backend

# ---- Runtime stage: just the binary + static bundle + the res/ data tree ----
FROM debian:bookworm-slim AS runtime
WORKDIR /app

COPY --from=builder /out/tmap-backend                /app/tmap-backend
COPY --from=builder /src/crates/frontend/dist        /app/dist
COPY --from=builder /src/res                          /app/res

# The backend reads these at startup; PORT is injected by Cloud Run (default 8080).
ENV TMAP_RES_DIR=/app/res \
    TMAP_DIST_DIR=/app/dist
EXPOSE 8080
CMD ["/app/tmap-backend"]
