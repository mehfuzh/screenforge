#!/usr/bin/env bash
# One-shot setup of the license backend: Stripe product/prices/webhook (live), Supabase schema,
# edge functions and secrets. Idempotent; rerun after fixing anything.
#
# Needs: supabase CLI (logged in), stripe CLI (logged in to the Smartloop account), gcloud
# (logged in; keys are read from Secret Manager in smartloop-gcp-us-east), jq.
# The Supabase database password is asked for by `supabase link` the first time
# (or export SUPABASE_DB_PASSWORD beforehand).
set -euo pipefail
cd "$(dirname "$0")"

REF=${SUPABASE_PROJECT_REF:-kxpppjdcqkxpybdawxum}
GCP_PROJECT=${GCP_PROJECT:-smartloop-gcp-us-east}
SITE=${SITE_URL:-https://screenforge.studio}
STATE_DIR="$HOME/Library/Application Support/ScreenForge"
KEYPAIR="$STATE_DIR/issuer-keypair.txt"      # written by `sf license keygen` (already done)
WHSEC_FILE="$STATE_DIR/stripe-webhook-secret.txt"

for t in supabase stripe gcloud jq; do command -v $t >/dev/null || { echo "missing: $t"; exit 1; }; done
[ -f "$KEYPAIR" ] || { echo "no issuer keypair at $KEYPAIR; run: cargo run -p sf-cli -- license keygen > \"$KEYPAIR\""; exit 1; }
SIGNING_KEY=$(grep '^SF_LICENSE_SIGNING_KEY=' "$KEYPAIR" | cut -d= -f2-)

sec() { gcloud secrets versions access latest --secret="$1" --project "$GCP_PROJECT"; }
echo "→ Reading API keys from GCP Secret Manager ($GCP_PROJECT)"
STRIPE_SECRET_KEY=${STRIPE_SECRET_KEY:-$(sec smartloop-gcp-us-east-prod-managed-stripe-api-key)}
RESEND_API_KEY=${RESEND_API_KEY:-$(sec smartloop-gcp-us-east-prod-managed-resend-api-key)}
[ -n "$STRIPE_SECRET_KEY" ] && [ -n "$RESEND_API_KEY" ] || { echo "could not read the Stripe / Resend keys"; exit 1; }

echo "→ Stripe (live): product"
PRODUCT_NAME="ScreenForge export license"
PROD=$(stripe products list --live --limit 100 | jq -r --arg n "$PRODUCT_NAME" '.data[] | select(.name == $n and .active) | .id' | head -1)
if [ -z "$PROD" ]; then
  PROD=$(stripe products create --live --name "$PRODUCT_NAME" \
    --description "Unlocks video export in ScreenForge. Recording and editing stay free." | jq -r .id)
  echo "  created $PROD"
else
  echo "  exists $PROD"
fi

price_by_key() { stripe prices list --live --limit 100 -d "lookup_keys[]=$1" | jq -r '.data[0].id // empty'; }
echo "→ Stripe (live): prices"
PM=$(price_by_key screenforge_monthly)
if [ -z "$PM" ]; then
  PM=$(stripe prices create --live --product "$PROD" --currency usd --unit-amount 1000 \
    -d "recurring[interval]=month" -d lookup_key=screenforge_monthly --nickname "ScreenForge monthly" | jq -r .id)
  echo "  created monthly $PM (US\$10 / month)"
else
  echo "  monthly $PM"
fi
PL=$(price_by_key screenforge_lifetime)
if [ -z "$PL" ]; then
  PL=$(stripe prices create --live --product "$PROD" --currency usd --unit-amount 2900 \
    -d lookup_key=screenforge_lifetime --nickname "ScreenForge lifetime" | jq -r .id)
  echo "  created lifetime $PL (US\$29 once)"
else
  echo "  lifetime $PL"
fi

echo "→ Stripe (live): webhook → Supabase"
WH_URL="https://$REF.supabase.co/functions/v1/stripe-webhook"
WH=$(stripe webhook_endpoints list --live --limit 100 | jq -r --arg u "$WH_URL" '.data[] | select(.url == $u) | .id' | head -1)
if [ -z "$WH" ]; then
  OUT=$(stripe webhook_endpoints create --live --url "$WH_URL" \
    -d "enabled_events[]=checkout.session.completed" -d "enabled_events[]=invoice.paid" \
    --description "ScreenForge licenses (Supabase edge function)")
  WH=$(echo "$OUT" | jq -r .id)
  echo "$OUT" | jq -r .secret > "$WHSEC_FILE"; chmod 600 "$WHSEC_FILE"
  echo "  created $WH (signing secret saved to $WHSEC_FILE)"
else
  echo "  exists $WH"
  [ -s "$WHSEC_FILE" ] || { echo "  its signing secret is not on this machine: reveal it at https://dashboard.stripe.com/webhooks/$WH and save it to $WHSEC_FILE, then rerun"; exit 1; }
fi
WHSEC=$(cat "$WHSEC_FILE")

echo "→ Supabase: link + schema"
if [ ! -f supabase/.temp/project-ref ] || [ "$(cat supabase/.temp/project-ref)" != "$REF" ]; then
  supabase link --project-ref "$REF"
fi
supabase db push

echo "→ Supabase: functions"
supabase functions deploy create-checkout license-refresh --project-ref "$REF"
supabase functions deploy stripe-webhook --no-verify-jwt --project-ref "$REF"

echo "→ Supabase: secrets"
supabase secrets set --project-ref "$REF" \
  SF_LICENSE_SIGNING_KEY="$SIGNING_KEY" \
  STRIPE_SECRET_KEY="$STRIPE_SECRET_KEY" \
  STRIPE_WEBHOOK_SECRET="$WHSEC" \
  STRIPE_PRICE_MONTHLY="$PM" \
  STRIPE_PRICE_LIFETIME="$PL" \
  RESEND_API_KEY="$RESEND_API_KEY" \
  LICENSE_FROM_EMAIL="ScreenForge <license@screenforge.studio>" \
  CHECKOUT_SUCCESS_URL="$SITE/success.html" \
  CHECKOUT_CANCEL_URL="$SITE/pricing.html"

echo "→ Smoke test: license-refresh for an unknown email should answer 404"
ANON=$(supabase projects api-keys --project-ref "$REF" -o json | jq -r '.[] | select(.name == "anon") | .api_key')
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "https://$REF.supabase.co/functions/v1/license-refresh" \
  -H "apikey: $ANON" -H "Authorization: Bearer $ANON" -H "Content-Type: application/json" -d '{"email":"nobody@example.com"}')
echo "  license-refresh → HTTP $CODE (expected 404)"

cat <<MSG

Done. Resend must have the sending domain verified for license@screenforge.studio
(Resend → Domains → add screenforge.studio, then add the DNS records to Cloudflare).
Buy flow: app → create-checkout → Stripe → stripe-webhook signs a key and emails it;
the app's Activate button fetches it by email.
MSG
