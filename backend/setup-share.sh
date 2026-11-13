#!/usr/bin/env bash
# Cloud share links (Pro): the R2 bucket the app uploads to, its public domain, and the
# Supabase side (schema, `share` function, secrets). Idempotent; rerun after fixing anything.
#
# Needs:
#   CLOUDFLARE_API_TOKEN  with Account · Workers R2 Storage · Edit and Zone · DNS · Edit
#                         (or in ~/Library/Application Support/ScreenForge/cloudflare-token)
#   R2_ACCESS_KEY_ID / R2_SECRET_ACCESS_KEY: an R2 API token (dashboard → R2 → Manage API
#                         tokens → Object Read & Write, this bucket only). The S3 credentials
#                         cannot be created from a token without that permission, so this is
#                         the one manual step.
#   supabase CLI (logged in), jq.
#
# The share page itself (functions/s/[id].js) ships with the site: scripts/deploy-site.sh or
# the Site workflow.
set -euo pipefail
cd "$(dirname "$0")"

REF=${SUPABASE_PROJECT_REF:-kxpppjdcqkxpybdawxum}
ACCOUNT_ID=${CLOUDFLARE_ACCOUNT_ID:-d0a3e9a6201a9989907bb9bcd4a82b19}
ZONE=${DOMAIN:-screenforge.studio}
BUCKET=${R2_BUCKET:-screenforge-shares}
MEDIA_HOST=${MEDIA_HOST:-media.$ZONE}
API=https://api.cloudflare.com/client/v4

TOKEN_FILE="$HOME/Library/Application Support/ScreenForge/cloudflare-token"
if [ -z "${CLOUDFLARE_API_TOKEN:-}" ] && [ -s "$TOKEN_FILE" ]; then CLOUDFLARE_API_TOKEN=$(cat "$TOKEN_FILE"); fi
: "${CLOUDFLARE_API_TOKEN:?set CLOUDFLARE_API_TOKEN (see the header of this script)}"
: "${R2_ACCESS_KEY_ID:?set R2_ACCESS_KEY_ID (an R2 API token, see the header of this script)}"
: "${R2_SECRET_ACCESS_KEY:?set R2_SECRET_ACCESS_KEY}"
for t in supabase jq curl; do command -v $t >/dev/null || { echo "missing: $t"; exit 1; }; done

cf() { curl -sS -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" -H "Content-Type: application/json" "$@"; }

echo "→ R2 bucket $BUCKET"
if cf "$API/accounts/$ACCOUNT_ID/r2/buckets/$BUCKET" | jq -e '.success == true' >/dev/null; then
  echo "  exists"
else
  cf -X POST "$API/accounts/$ACCOUNT_ID/r2/buckets" --data "{\"name\":\"$BUCKET\"}" | jq -e '.success == true' >/dev/null || { echo "  creating the bucket failed (does the token have R2 Edit?)"; exit 1; }
  echo "  created"
fi

echo "→ Public domain $MEDIA_HOST → $BUCKET"
ZONE_ID=$(cf "$API/zones?name=$ZONE" | jq -r '.result[0].id // empty')
[ -n "$ZONE_ID" ] || { echo "  zone $ZONE not found on this account"; exit 1; }
if cf "$API/accounts/$ACCOUNT_ID/r2/buckets/$BUCKET/domains/custom" | jq -e --arg h "$MEDIA_HOST" '.result.domains[]? | select(.domain == $h)' >/dev/null; then
  echo "  attached"
else
  cf -X POST "$API/accounts/$ACCOUNT_ID/r2/buckets/$BUCKET/domains/custom" \
    --data "{\"domain\":\"$MEDIA_HOST\",\"zoneId\":\"$ZONE_ID\",\"enabled\":true,\"minTLS\":\"1.2\"}" | jq -e '.success == true' >/dev/null \
    && echo "  attached (the certificate takes a few minutes)" || { echo "  attaching the domain failed"; exit 1; }
fi

echo "→ Supabase: schema, secrets, share function"
supabase db push --project-ref "$REF" 2>/dev/null || supabase db push
supabase secrets set --project-ref "$REF" \
  R2_ACCOUNT_ID="$ACCOUNT_ID" R2_BUCKET="$BUCKET" \
  R2_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID" R2_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY" \
  SHARE_MEDIA_URL="https://$MEDIA_HOST" SHARE_PAGE_URL="https://$ZONE/s/"
supabase functions deploy share --project-ref "$REF"

cat <<MSG

Done. Videos are served from https://$MEDIA_HOST; links open https://$ZONE/s/<id> once the
site is deployed with functions/ (scripts/deploy-site.sh, or push to main).
MSG
