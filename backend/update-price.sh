#!/usr/bin/env bash
# Change the price of the one-time ScreenForge export license, end to end:
#   Stripe (live): new price with the amount, which takes over the `screenforge_lifetime` lookup
#                  key and becomes the product's default; the old price is archived; the retired
#                  US$10 monthly price is archived if it is still active.
#   Supabase:      STRIPE_PRICE_LIFETIME secret → the new price id; create-checkout redeployed.
#
#   backend/update-price.sh 4900            # US$49 once (cents)
#   backend/update-price.sh --stripe-only 4900   # Stripe part only; prints the price id (setup.sh)
#
# Idempotent: when the live price already has this amount nothing is created, the secret is
# still (re)set and the function redeployed. Needs: stripe CLI (logged in to the Smartloop
# account), jq; supabase CLI (logged in) unless --stripe-only.
set -euo pipefail
cd "$(dirname "$0")"   # the supabase CLI looks for supabase/functions next to the script

STRIPE_ONLY=0
if [ "${1:-}" = "--stripe-only" ]; then STRIPE_ONLY=1; shift; fi
CENTS=${1:-}
[[ "$CENTS" =~ ^[0-9]+$ ]] && [ "$CENTS" -ge 100 ] || { echo "usage: $0 [--stripe-only] <amount in US cents, e.g. 4900>" >&2; exit 2; }
REF=${SUPABASE_PROJECT_REF:-kxpppjdcqkxpybdawxum}
PRODUCT_NAME="ScreenForge export license"
LOOKUP=screenforge_lifetime
LABEL="US\$$((CENTS / 100))$([ $((CENTS % 100)) -ne 0 ] && printf '.%02d' $((CENTS % 100)) || true) once"

for t in stripe jq; do command -v $t >/dev/null || { echo "missing: $t" >&2; exit 1; }; done
[ $STRIPE_ONLY -eq 1 ] || command -v supabase >/dev/null || { echo "missing: supabase" >&2; exit 1; }
log() { echo "$@" >&2; }

price_by_key() { stripe prices list --live --limit 100 -d "lookup_keys[]=$1" | jq -r '.data[0].id // empty'; }
price_field() { stripe prices retrieve --live "$1" | jq -r ".$2"; }

log "→ Stripe (live): product"
PROD=$(stripe products list --live --limit 100 | jq -r --arg n "$PRODUCT_NAME" '.data[] | select(.name == $n and .active) | .id' | head -1)
[ -n "$PROD" ] || { log "no active product named '$PRODUCT_NAME'; run backend/setup.sh first"; exit 1; }
log "  $PROD"

log "→ Stripe (live): retired monthly price"
PM=$(price_by_key screenforge_monthly)
if [ -n "$PM" ] && [ "$(price_field "$PM" active)" = "true" ]; then
  stripe prices update --live "$PM" -d active=false >/dev/null
  log "  archived $PM"
else
  log "  ${PM:-none}${PM:+ (already archived)}"
fi

log "→ Stripe (live): lifetime price ($LABEL)"
PL=$(price_by_key "$LOOKUP")
if [ -n "$PL" ] && [ "$(price_field "$PL" unit_amount)" = "$CENTS" ] && [ "$(price_field "$PL" active)" = "true" ]; then
  log "  $PL already $LABEL"
else
  OLD_PL=$PL
  PL=$(stripe prices create --live -d product="$PROD" -d currency=usd -d unit_amount="$CENTS" \
    -d lookup_key="$LOOKUP" -d transfer_lookup_key=true -d nickname="ScreenForge lifetime" | jq -r .id)
  log "  created $PL"
  if [ -n "$OLD_PL" ] && [ "$(price_field "$OLD_PL" active)" = "true" ]; then
    stripe prices update --live "$OLD_PL" -d active=false >/dev/null
    log "  archived $OLD_PL ($(price_field "$OLD_PL" unit_amount)¢)"
  fi
fi
stripe products update --live "$PROD" -d default_price="$PL" >/dev/null
log "  default price of $PROD → $PL"

if [ $STRIPE_ONLY -eq 1 ]; then
  echo "$PL"
  exit 0
fi

log "→ Supabase ($REF): STRIPE_PRICE_LIFETIME + create-checkout"
supabase secrets set --project-ref "$REF" STRIPE_PRICE_LIFETIME="$PL" >&2
supabase functions deploy create-checkout --project-ref "$REF" >&2

log ""
log "Live checkout now charges $LABEL ($PL)."
log "Remaining by hand: the price text in site/*.html, README.md, backend/README.md and crates/sf-license/src/lib.rs"
log "(grep -rn 'US\$' site README.md backend/README.md crates/sf-license/src/lib.rs), then commit and deploy the site."
