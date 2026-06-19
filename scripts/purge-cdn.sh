#!/usr/bin/env bash
#
# Purge the Cloudflare edge cache. Run after a deploy so a data change goes live
# immediately instead of waiting for the edge TTL to expire — the data endpoints
# advertise a long shared-cache TTL (s-maxage, see serve_cached in main.rs), which
# is only safe because we invalidate here on every deploy.
#
# Standalone-runnable, and called by deploy.sh as its last step. Needs two values
# (in scripts/deploy.env, gitignored, or the environment):
#   CF_ZONE_ID   — the Cloudflare zone id for callistoflight.com
#   CF_API_TOKEN — an API token scoped to Zone › Cache Purge › Purge for that zone
# If either is unset the purge is skipped with a notice (so a deploy still works
# before the CDN is wired up). See DEPLOY.md › "CDN (Cloudflare)".
#
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
if [[ -f "$SCRIPT_DIR/deploy.env" ]]; then set -a; source "$SCRIPT_DIR/deploy.env"; set +a; fi

if [[ -z "${CF_ZONE_ID:-}" || -z "${CF_API_TOKEN:-}" ]]; then
  echo ">> CDN purge skipped (CF_ZONE_ID / CF_API_TOKEN not set in scripts/deploy.env)."
  exit 0
fi

echo ">> purging Cloudflare edge cache (zone $CF_ZONE_ID)…"
resp="$(curl -sS -X POST \
  "https://api.cloudflare.com/client/v4/zones/${CF_ZONE_ID}/purge_cache" \
  -H "Authorization: Bearer ${CF_API_TOKEN}" \
  -H "Content-Type: application/json" \
  --data '{"purge_everything":true}')"

# The API always returns 200 with a JSON {"success":true|false,...}; check it.
if printf '%s' "$resp" | grep -q '"success":true'; then
  echo ">> CDN cache purged."
else
  echo ">> CDN purge FAILED: $resp" >&2
  exit 1
fi
