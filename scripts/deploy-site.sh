#!/usr/bin/env bash
# Publish the landing page (site/) on Cloudflare Pages under a custom domain.
#
# One-time setup, done by hand in the Cloudflare dashboard (no API for either):
#   1. Buy the domain: https://dash.cloudflare.com/?to=/:account/domains/register
#      (default: screenforge.studio; override with DOMAIN=...)
#   2. Create an API token: https://dash.cloudflare.com/profile/api-tokens → "Create Token"
#      → "Create Custom Token" with these permissions:
#        Account · Cloudflare Pages · Edit
#        Zone    · DNS             · Edit      (zone: the domain above)
#        Zone    · Zone            · Read
#
# Then:
#   export CLOUDFLARE_API_TOKEN=...   # from step 2
#   scripts/deploy-site.sh            # safe to rerun; every step is idempotent
#
# Afterwards add the same token as the GitHub secret CLOUDFLARE_API_TOKEN (and
# CLOUDFLARE_ACCOUNT_ID, printed below) so .github/workflows/site.yml deploys on every push.
set -euo pipefail

DOMAIN=${DOMAIN:-screenforge.studio}
PROJECT=${PROJECT:-screenforge}
SITE_DIR=${SITE_DIR:-site}
API=https://api.cloudflare.com/client/v4

TOKEN_FILE="$HOME/Library/Application Support/ScreenForge/cloudflare-token"
if [ -z "${CLOUDFLARE_API_TOKEN:-}" ] && [ -s "$TOKEN_FILE" ]; then CLOUDFLARE_API_TOKEN=$(cat "$TOKEN_FILE"); fi
export CLOUDFLARE_API_TOKEN
: "${CLOUDFLARE_API_TOKEN:?set CLOUDFLARE_API_TOKEN or put it in $TOKEN_FILE (see the header of this script)}"
command -v jq >/dev/null || { echo "jq is required (brew install jq)"; exit 1; }
command -v npx >/dev/null || { echo "node/npx is required for wrangler"; exit 1; }
[ -f "$SITE_DIR/index.html" ] || { echo "run from the repo root; $SITE_DIR/index.html not found"; exit 1; }

cf() { curl -sS -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" -H "Content-Type: application/json" "$@"; }
ok() { jq -e '.success == true' >/dev/null; }

echo "→ Checking token"
cf "$API/user/tokens/verify" | jq -e '.result.status == "active"' >/dev/null || { echo "token is not active"; exit 1; }

# the Smartloop account (the deploy token has no account-listing permission)
ACCOUNT_ID=${CLOUDFLARE_ACCOUNT_ID:-d0a3e9a6201a9989907bb9bcd4a82b19}
[ -n "$ACCOUNT_ID" ] && [ "$ACCOUNT_ID" != null ] || { echo "could not determine the account id; set CLOUDFLARE_ACCOUNT_ID"; exit 1; }
echo "  account $ACCOUNT_ID"

echo "→ Looking up zone $DOMAIN"
ZONE_ID=$(cf "$API/zones?name=$DOMAIN" | jq -r '.result[0].id // empty')
if [ -z "$ZONE_ID" ]; then
  echo "  $DOMAIN is not on this Cloudflare account yet: deploying to $PROJECT.pages.dev only."
  echo "  Buy it at https://dash.cloudflare.com/$ACCOUNT_ID/domains/register?domain=$DOMAIN and rerun to attach it."
else
  echo "  zone $ZONE_ID"
fi

echo "→ Pages project $PROJECT"
if ! cf "$API/accounts/$ACCOUNT_ID/pages/projects/$PROJECT" | ok; then
  cf -X POST "$API/accounts/$ACCOUNT_ID/pages/projects" \
    --data "{\"name\":\"$PROJECT\",\"production_branch\":\"main\"}" | ok || { echo "creating the Pages project failed"; exit 1; }
  echo "  created"
else
  echo "  exists"
fi

# the project's real *.pages.dev host (the plain name may be taken, e.g. screenforge-7nw.pages.dev)
PAGES_HOST=$(cf "$API/accounts/$ACCOUNT_ID/pages/projects/$PROJECT" | jq -r '.result.subdomain // empty')
PAGES_HOST=${PAGES_HOST:-$PROJECT.pages.dev}
echo "  $PAGES_HOST"

echo "→ Deploying $SITE_DIR"
CLOUDFLARE_ACCOUNT_ID=$ACCOUNT_ID npx --yes wrangler@4 pages deploy "$SITE_DIR" --project-name "$PROJECT" --branch main --commit-dirty=true

if [ -n "$ZONE_ID" ]; then
echo "→ Custom domains"
for host in "$DOMAIN" "www.$DOMAIN"; do
  if cf "$API/accounts/$ACCOUNT_ID/pages/projects/$PROJECT/domains" | jq -e --arg h "$host" '.result[] | select(.name == $h)' >/dev/null; then
    echo "  $host already attached"
  else
    cf -X POST "$API/accounts/$ACCOUNT_ID/pages/projects/$PROJECT/domains" --data "{\"name\":\"$host\"}" | ok && echo "  attached $host" || echo "  could not attach $host (will retry on rerun)"
  fi
done

echo "→ DNS records → $PAGES_HOST (proxied)"
upsert_cname() {
  local name=$1
  local body="{\"type\":\"CNAME\",\"name\":\"$name\",\"content\":\"$PAGES_HOST\",\"proxied\":true,\"ttl\":1,\"comment\":\"ScreenForge landing page (scripts/deploy-site.sh)\"}"
  # remove placeholder A/AAAA records the registrar may have created at this name
  cf "$API/zones/$ZONE_ID/dns_records?name=$name" | jq -r '.result[] | select(.type == "A" or .type == "AAAA") | .id' | while read -r id; do
    [ -n "$id" ] && cf -X DELETE "$API/zones/$ZONE_ID/dns_records/$id" >/dev/null && echo "  removed stale A/AAAA at $name"
  done
  local existing
  existing=$(cf "$API/zones/$ZONE_ID/dns_records?name=$name&type=CNAME" | jq -r '.result[0].id // empty')
  if [ -n "$existing" ]; then
    cf -X PUT "$API/zones/$ZONE_ID/dns_records/$existing" --data "$body" | ok && echo "  updated $name"
  else
    cf -X POST "$API/zones/$ZONE_ID/dns_records" --data "$body" | ok && echo "  created $name"
  fi
}
upsert_cname "$DOMAIN"
upsert_cname "www.$DOMAIN"

echo "→ Always use HTTPS"
cf -X PATCH "$API/zones/$ZONE_ID/settings/always_use_https" --data '{"value":"on"}' | ok || true

fi

cat <<MSG

Done. The site is live at https://$PAGES_HOST${ZONE_ID:+ and, once the certificate is issued
(usually a few minutes), at https://$DOMAIN and https://www.$DOMAIN}.

For automatic deploys on push, add these repository secrets on GitHub:
  gh secret set CLOUDFLARE_API_TOKEN  -R mehfuzh/screenforge
  gh secret set CLOUDFLARE_ACCOUNT_ID -R mehfuzh/screenforge --body "$ACCOUNT_ID"
MSG
